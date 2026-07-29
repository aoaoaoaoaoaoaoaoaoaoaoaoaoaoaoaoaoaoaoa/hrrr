use crate::{
    basemap::{self, Basemap, TileKey, VectorTile},
    cache::Custodian,
    config::{Config, ConfigLoad},
    fold_ui, forge,
    library::EntryName,
    library_ui::{self, Action as ViewAction, NameEdit, ShelfEdit},
    map::{self, FieldPaint},
    model::{FieldGrid, FrameKey, LeadHour, MercatorPoint, Product, RunId, RunSelection, Viewport},
    spec::{Scale, ScaleAtlas, SmokeRegime, TemperatureSeason},
    state::Slate,
    vector_map::VectorPaint,
    view::{SavedView, ViewLibrary, ViewSlot},
    worker::{Command, DemandId, Event, LoadDemand, LoadIntent, Worker},
    xdg::{InstanceGuard, Lair},
};
use anyhow::Result;
use dwemer_poolrooms::{
    chrome,
    water::{Domain, Frame as WaterFrame, Surface, Wetness},
};
use egui::Color32;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

const STATE_SETTLE: Duration = Duration::from_millis(450);
const FRONTIER_POLL: Duration = Duration::from_mins(1);
const FIELD_CAPACITY: usize = 12;
const VECTOR_CEILING: usize = 512 * 1_048_576;
const LATEST_RUN: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::R);
const LATEST_LONG_RUN: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
    egui::Key::R,
);
#[derive(Clone, Copy, PartialEq)]
struct SmokeScene {
    key: FrameKey,
    viewport: Viewport,
    extent: [f32; 2],
}

#[derive(Default)]
struct SmokeSurvey {
    scene: Option<SmokeScene>,
    peak: Option<f32>,
}

#[derive(Default)]
struct PinGesture {
    captured: bool,
    hot: Option<usize>,
}

#[derive(Clone, Copy)]
struct PinTug {
    slot: usize,
    origin: MercatorPoint,
    world_points: f64,
}

impl SmokeSurvey {
    fn discern(
        &mut self,
        key: FrameKey,
        field: &FieldGrid,
        viewport: Viewport,
        rect: egui::Rect,
    ) -> Option<f32> {
        let scene = SmokeScene {
            key,
            viewport,
            extent: [rect.width(), rect.height()],
        };
        if self.scene != Some(scene) {
            self.scene = Some(scene);
            self.peak = map::visible_peak(field, viewport, rect);
        }
        self.peak
    }
}

pub struct WeatherApp {
    lair: Lair,
    _instance: InstanceGuard,
    config: Config,
    views: ViewLibrary,
    slate: Slate,
    active_view: EntryName,
    viewport: Viewport,
    pins: Vec<MercatorPoint>,
    run: Option<RunId>,
    worker: Worker,
    basemap: Basemap,
    custodian: Custodian,
    latest_run: Option<RunId>,
    run_extents: HashMap<RunId, LeadHour>,
    surveying_run: Option<RunId>,
    next_survey: Instant,
    announced_discovery: bool,
    demand_id: DemandId,
    loading: Option<LoadDemand>,
    fields: FrameBank,
    displayed_field: Option<(FrameKey, Arc<FieldGrid>)>,
    prefetch: VecDeque<FrameKey>,
    tiles: VectorBank,
    presented_basemap: Arc<[Arc<VectorTile>]>,
    tile_inflight: HashSet<TileKey>,
    tile_faults: HashSet<TileKey>,
    fold_focus: fold_ui::FoldCage,
    transient_probe: Option<MercatorPoint>,
    pin_tug: Option<PinTug>,
    view_name_entry: String,
    name_edit: NameEdit,
    shelf_edit: Option<ShelfEdit>,
    scales: ScaleAtlas,
    smoke_regime: SmokeRegime,
    smoke_survey: SmokeSurvey,
    water: Surface,
    dirty: Option<Instant>,
    config_dirty: Option<Instant>,
    views_dirty: Option<Instant>,
    status: String,
    basemap_status: String,
}

impl WeatherApp {
    pub fn open(ctx: &egui::Context) -> Result<Self> {
        let lair = Lair::claim()?;
        let instance = lair.lock_instance()?;
        let ConfigLoad {
            config,
            legacy_views,
        } = Config::load(&lair.config_path())?;
        let had_legacy_views = legacy_views.is_some();
        let (mut views, migrated_views) = ViewLibrary::load(&lair.views_path(), legacy_views)?;
        let (mut slate, migrated_slate) = Slate::load(&lair.slate_path())?;
        views.restore_folds(&slate.closed_folders);
        let active_view = views
            .active(slate.active_view.clone())
            .or_else(|| views.all().next().map(|view| view.name.clone()));
        let Some(active_view) = active_view else {
            anyhow::bail!("view library admitted no active document");
        };
        let Some(view) = views.get(&active_view) else {
            anyhow::bail!("active view `{active_view}` vanished during startup");
        };
        let viewport = view.viewport;
        let pins = view.pins.clone();
        let run = slate.cycle.fixed();
        slate.active_view = Some(active_view.clone());
        let worker = Worker::spawn(ctx.clone(), &lair)?;
        let basemap = Basemap::spawn(ctx.clone(), lair.basemap_path()?)?;
        let custodian = Custodian::spawn(ctx.clone(), lair.cache_manager())?;
        let mut water = Surface::new(Wetness::Wet);
        {
            let (chemistry, agitation) = water.laboratory_mut();
            chemistry.refract_px = 0.34;
            chemistry.meniscus_px = 0.62;
            chemistry.ior_spread = 0.14;
            chemistry.bulge_px = 3.2;
            chemistry.source_gain = 18.0;
            agitation.enter_impulse = 0.22;
            agitation.exit_impulse = 0.12;
            agitation.click_impulse = 0.62;
            agitation.scroll_coupling = 0.006;
            agitation.pond_impulse = 0.4;
        }
        let app = Self {
            lair,
            _instance: instance,
            config,
            views,
            slate,
            active_view,
            viewport,
            pins,
            run,
            worker,
            basemap,
            custodian,
            latest_run: None,
            run_extents: HashMap::new(),
            surveying_run: None,
            next_survey: Instant::now(),
            announced_discovery: true,
            demand_id: DemandId::default(),
            loading: None,
            fields: FrameBank::new(FIELD_CAPACITY),
            displayed_field: None,
            prefetch: VecDeque::new(),
            tiles: VectorBank::new(VECTOR_CEILING),
            presented_basemap: Arc::from([]),
            tile_inflight: HashSet::new(),
            tile_faults: HashSet::new(),
            fold_focus: fold_ui::FoldCage::default(),
            transient_probe: None,
            pin_tug: None,
            view_name_entry: String::new(),
            name_edit: NameEdit::Idle,
            shelf_edit: None,
            scales: ScaleAtlas::default(),
            smoke_regime: SmokeRegime::default(),
            smoke_survey: SmokeSurvey::default(),
            water,
            dirty: migrated_slate.then(Instant::now),
            config_dirty: had_legacy_views.then(Instant::now),
            views_dirty: migrated_views.then(Instant::now),
            status: "finding the newest complete HRRR cycle…".to_owned(),
            basemap_status: "OPENING PROTOMAPS ARCHIVE".to_owned(),
        };
        app.worker.send(Command::Discover)?;
        Ok(app)
    }

    pub fn pulse(&mut self, ui: &mut egui::Ui) {
        self.absorb_events(ui.ctx());
        self.tend_survey(ui.ctx());
        self.fold_focus.take_keys(ui.ctx());
        self.take_keys(ui.ctx());
        self.fold_focus.begin_pass();
        let _left = egui::Panel::left("forecast-inspector")
            .resizable(false)
            .exact_size(chrome::INSPECTOR_WIDTH)
            .show_inside(ui, |ui| {
                let _scroll = egui::ScrollArea::vertical()
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(ui.spacing().item_spacing.x);
                        self.inspector(ui);
                    });
            });
        let _center = egui::CentralPanel::default().show_inside(ui, |ui| self.map(ui));
        self.fold_focus.end_pass();
        self.flush_state(false);
        self.flush_config(false);
        self.flush_views(false);
    }

    pub fn water_frame(
        &mut self,
        ctx: &egui::Context,
        pixels_per_point: f32,
        tooltip_rects: &[egui::Rect],
    ) -> WaterFrame {
        self.water.frame(ctx, pixels_per_point, tooltip_rects, None)
    }

    pub fn retire(&mut self) {
        self.flush_config(true);
        self.flush_state(true);
        self.flush_views(true);
    }

    pub const fn close_minimizes(&self) -> bool {
        self.config.close_minimizes
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let mut chosen_product = None;
        let (wake, focus) = fold_ui::section(ui, "product", "field", true, |ui| {
            for row in Product::ROWS {
                let _row = ui.horizontal(|ui| {
                    let spacing = ui.spacing().item_spacing.x * row.len().saturating_sub(1) as f32;
                    let width = (ui.available_width() - spacing) / row.len() as f32;
                    for &product in row {
                        let response = ui.add_sized(
                            [width, 26.0],
                            chrome::glyph_button(
                                product.label(),
                                self.slate.overlay.active() == Some(product),
                            ),
                        );
                        chrome::tension(ui, &response);
                        if response.hovered() {
                            self.water.hover(("product", product), response.rect);
                        }
                        if response.clicked() {
                            chosen_product = Some((product, response.rect));
                        }
                    }
                });
            }
        });
        self.fold_focus.record(focus);
        self.water.fold(wake);
        if let Some((product, rect)) = chosen_product {
            self.water.select(rect);
            self.strike_overlay(product);
        }

        let (wake, focus) = fold_ui::section(ui, "forecast", "forecast", true, |ui| {
            self.forecast_controls(ui);
        });
        self.fold_focus.record(focus);
        self.water.fold(wake);

        let (wake, focus) = fold_ui::section(ui, "active-view", "active view", true, |ui| {
            self.active_view_panel(ui);
        });
        self.fold_focus.record(focus);
        self.water.fold(wake);

        let (wake, focus) = fold_ui::section(ui, "view-library", "views", true, |ui| {
            self.view_library_panel(ui);
        });
        self.fold_focus.record(focus);
        self.water.fold(wake);

        let mut reset = None;
        let (wake, focus) = fold_ui::section(ui, "navigation", "navigation", true, |ui| {
            let row = ui.horizontal(|ui| {
                let _label = ui.label(chrome::muted("SCROLL"));
                let _value = ui.label("zoom at pointer");
            });
            let _row = row;
            let row = ui.horizontal(|ui| {
                let _label = ui.label(chrome::muted("DRAG"));
                let _value = ui.label("pan map");
            });
            let _row = row;
            let response = ui.add_sized(
                [ui.available_width(), 24.0],
                chrome::glyph_button("⌖  RESET CONUS", false),
            );
            chrome::tension(ui, &response);
            if response.hovered() {
                self.water.hover("reset-view", response.rect);
            }
            if response.clicked() {
                reset = Some(response.rect);
            }
            let _select = ui.horizontal(|ui| {
                let _key = ui.label(chrome::muted("0–9"));
                let _meaning = ui.label("select bound view");
            });
            let _assign = ui.horizontal(|ui| {
                let _key = ui.label(chrome::muted("SHIFT+0–9"));
                let _meaning = ui.label("bind active view");
            });
        });
        self.fold_focus.record(focus);
        self.water.fold(wake);
        if let Some(rect) = reset {
            self.viewport = Viewport::default();
            self.sync_active_view();
            self.water.click(rect);
        }

        let mut toggle_close = None;
        let (wake, focus) = fold_ui::section(ui, "application", "application", true, |ui| {
            let response = ui.add_sized(
                [ui.available_width(), 26.0],
                chrome::glyph_button("CLOSE MINIMIZES", self.config.close_minimizes),
            );
            chrome::tension(ui, &response);
            if response.hovered() {
                self.water.hover("close-minimizes", response.rect);
            }
            if response.clicked() {
                toggle_close = Some(response.rect);
            }
            let law = if self.config.close_minimizes {
                "window close hides to tray"
            } else {
                "window close terminates"
            };
            let _law = ui.label(chrome::muted(law));
        });
        self.fold_focus.record(focus);
        self.water.fold(wake);
        if let Some(rect) = toggle_close {
            self.config.close_minimizes = !self.config.close_minimizes;
            let status = if self.config.close_minimizes {
                "window close will minimize to tray"
            } else {
                "window close will terminate"
            };
            status.clone_into(&mut self.status);
            self.mark_config_dirty();
            self.water.select(rect);
        }

        let (wake, focus) = fold_ui::section(ui, "status", "status", true, |ui| {
            let _status = ui.label(chrome::muted(&self.status));
            ui.add_space(3.0);
            let _source = ui.label(chrome::muted(format!(
                "DATA · NOAA HRRR\nBASE · {}",
                self.basemap_status
            )));
        });
        self.fold_focus.record(focus);
        self.water.fold(wake);
    }

    fn active_view_panel(&mut self, ui: &mut egui::Ui) {
        let mut edit = self.name_edit;
        let actions = library_ui::active_card(
            ui,
            "view",
            &mut self.view_name_entry,
            &mut edit,
            &self.active_view,
        );
        self.name_edit = edit;
        self.apply_view_actions(actions);
    }

    fn view_library_panel(&mut self, ui: &mut egui::Ui) {
        let mut shelf_edit = self.shelf_edit.take();
        let actions =
            library_ui::library(ui, "view", &self.active_view, &self.views, &mut shelf_edit);
        self.shelf_edit = shelf_edit;
        self.apply_view_actions(actions);
    }

    fn forecast_controls(&mut self, ui: &mut egui::Ui) {
        let Some(run) = self.run else {
            let _waiting = ui.label(chrome::muted("awaiting cycle index"));
            return;
        };
        let run_label = run
            .local_label()
            .unwrap_or_else(|_| "invalid cycle time".to_owned());
        let valid_label = run
            .valid_local_label(self.slate.lead)
            .unwrap_or_else(|_| "invalid valid time".to_owned());
        let _run = ui.label(chrome::eyebrow(format!("RUN · {run_label}")));
        let _valid = ui.label(chrome::section_title(valid_label));
        ui.add_space(3.0);

        let horizon = run.horizon().unwrap_or(LeadHour::ZERO);
        let published = self.run_extents.get(&run).copied();
        let gate = published.unwrap_or(LeadHour::ZERO);
        let mut step = None;
        let _row = ui.horizontal(|ui| {
            let previous = ui.add_enabled(
                published.is_some() && self.slate.lead > LeadHour::ZERO,
                chrome::glyph_button("◀", false),
            );
            chrome::tension(ui, &previous);
            if previous.clicked() {
                step = Some((self.slate.lead.saturating_previous(), previous.rect));
            }
            let _lead = ui.label(chrome::section_title(format!(
                "{} · F{:02}/F{:02}",
                self.slate.lead,
                gate.get(),
                horizon.get()
            )));
            let next = ui.add_enabled(
                published.is_some() && self.slate.lead < gate,
                chrome::glyph_button("▶", false),
            );
            chrome::tension(ui, &next);
            if next.clicked() {
                step = Some((self.slate.lead.saturating_next(gate), next.rect));
            }
        });
        if let Some((lead, rect)) = step {
            self.choose_lead(lead);
            self.water.lever(rect, 1.0);
        }
        let mut raw_lead = u16::from(self.slate.lead.get());
        let rail = ui
            .add_enabled_ui(published.is_some(), |ui| {
                chrome::Rail::new(&mut raw_lead, 0..=u16::from(horizon.get()))
                    .allowed(0..=u16::from(gate.get()))
                    .detents(u16::from(horizon.get()) + 1)
                    .show(ui)
            })
            .inner;
        self.water.rail(&rail);
        if rail.changed()
            && published.is_some()
            && let Ok(raw_lead) = u8::try_from(raw_lead)
            && let Ok(lead) = LeadHour::forge(raw_lead)
        {
            self.choose_lead(lead);
        }
        if published.is_none() {
            let _surveying = ui.label(chrome::muted("surveying publication frontier…"));
        }

        ui.add_space(4.0);
        let latest_extended = self
            .latest_run
            .map(|latest| RunSelection::LatestLong.bind(latest));
        let mut run_step = None;
        let _row = ui.horizontal(|ui| {
            let older = ui.add(chrome::glyph_button("−1H", false));
            if older.clicked() {
                run_step = Some((RunSelection::Fixed(run.hours_ago(1)), older.rect));
            }
            let latest = ui
                .add_enabled(
                    self.latest_run.is_some(),
                    chrome::glyph_button(
                        "LATEST",
                        self.slate.cycle == RunSelection::Latest && self.latest_run == Some(run),
                    ),
                )
                .on_hover_text(format!(
                    "Latest run · {}",
                    ui.ctx().format_shortcut(&LATEST_RUN)
                ));
            if latest.clicked() {
                run_step = Some((RunSelection::Latest, latest.rect));
            }
            let latest_long = ui
                .add_enabled(
                    latest_extended.is_some(),
                    chrome::glyph_button(
                        "LATEST LONG",
                        self.slate.cycle == RunSelection::LatestLong
                            && latest_extended == Some(run),
                    ),
                )
                .on_hover_text(format!(
                    "Latest extended run · {}",
                    ui.ctx().format_shortcut(&LATEST_LONG_RUN)
                ));
            if latest_long.clicked() {
                run_step = Some((RunSelection::LatestLong, latest_long.rect));
            }
            let newer = ui.add_enabled(
                self.latest_run.is_some_and(|latest_run| run < latest_run),
                chrome::glyph_button("+1H", false),
            );
            if newer.clicked() {
                let candidate = run.hours_after(1);
                run_step = self
                    .latest_run
                    .map(|latest| (RunSelection::Fixed(candidate.min(latest)), newer.rect));
            }
        });
        if let Some((selection, rect)) = run_step {
            self.follow_cycle(selection);
            self.water.click(rect);
        }
    }

    fn map(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        self.water.begin(Domain::shelf(rect));
        let pins = self.tug_pins(ui, rect);
        self.navigate(ui, &response, rect, pins.captured);
        let painter = ui.painter_at(rect);
        let _ground = painter.rect_filled(
            rect,
            0.0,
            Color32::from_rgb(
                basemap::PAPER_SRGB[0],
                basemap::PAPER_SRGB[1],
                basemap::PAPER_SRGB[2],
            ),
        );

        let cover = basemap::cover(self.viewport, rect);
        self.demand_cover(&cover);
        let coherent = cover
            .finest_ready(|key| self.tiles.contains(key))
            .map(|stratum| stratum.keys.clone());
        if let Some(keys) = coherent
            && (keys.len() != self.presented_basemap.len()
                || keys
                    .iter()
                    .zip(self.presented_basemap.iter())
                    .any(|(key, tile)| *key != tile.key))
        {
            self.presented_basemap = keys
                .into_iter()
                .filter_map(|key| self.tiles.get(key).cloned())
                .collect();
        }
        let bounds = map::world_bounds(self.viewport, rect).map(|v| v as f32);
        if !self.presented_basemap.is_empty() {
            let _basemap = painter.add(egui_wgpu::Callback::new_paint_callback(
                rect,
                VectorPaint {
                    tiles: self.presented_basemap.clone(),
                    center_world: self.viewport.center_mercator,
                    world_points: map::world_pixels(self.viewport) as f32,
                    viewport_points: [rect.width(), rect.height()],
                    view_zoom: self.viewport.zoom as f32,
                    apparition_span: basemap::APPARITION_SPAN,
                },
            ));
        }

        let painted_field = self.active_field().or_else(|| self.displayed_field.clone());
        let mut legend_scale = None;
        if let Some((key, field)) = painted_field {
            if key.product == Product::Smoke {
                let peak = self
                    .smoke_survey
                    .discern(key, &field, self.viewport, rect)
                    .map(|raw| self.scale_for(key).unit.convert(raw));
                self.smoke_regime = self.smoke_regime.reckon(peak);
            }
            let scale = self.scale_for(key).clone();
            legend_scale = Some(scale.clone());
            let _field = painter.add(egui_wgpu::Callback::new_paint_callback(
                rect,
                FieldPaint {
                    key,
                    field,
                    scale,
                    world_bounds: bounds,
                },
            ));
        }
        self.paint_labels(&painter, rect);
        if let Some(scale) = legend_scale.as_ref() {
            Self::legend(&painter, rect, scale);
        }
        let _edge = painter.rect_stroke(
            rect.shrink(0.5),
            0.0,
            egui::Stroke::new(1.0_f32, chrome::EDGE_STRONG),
            egui::StrokeKind::Inside,
        );
        if self.loading.is_some_and(|demand| {
            demand.intent == LoadIntent::Foreground(self.demand_id)
                && Some(demand.key) == self.active_key()
        }) {
            self.water.show_loading(ui.ctx(), rect);
        } else {
            self.water.hide_loading();
        }
        self.show_marks(ui.ctx(), &painter, rect, pins.hot);
    }

    fn navigate(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        pin_captured: bool,
    ) {
        let before = self.viewport;
        let minimum_zoom = map::minimum_zoom(rect.height());
        self.viewport.zoom = self.viewport.zoom.max(minimum_zoom);
        if !pin_captured && response.dragged_by(egui::PointerButton::Primary) {
            let delta = ui.input(|input| input.pointer.delta());
            let scale = map::world_pixels(self.viewport);
            self.viewport.center_mercator[0] -= f64::from(delta.x) / scale;
            self.viewport.center_mercator[1] -= f64::from(delta.y) / scale;
            self.water.drag(rect, delta.y);
        }
        if let Some((scroll, pointer)) = ui.input(|input| {
            input
                .pointer
                .hover_pos()
                .filter(|pointer| rect.contains(*pointer))
                .map(|pointer| (input.smooth_scroll_delta.y, pointer))
        }) && scroll.abs() > f32::EPSILON
        {
            let anchor = map::world_at(self.viewport, rect, pointer);
            self.viewport.zoom = (self.viewport.zoom + f64::from(scroll) * 0.008)
                .clamp(minimum_zoom, Viewport::MAX_ZOOM);
            let scale = map::world_pixels(self.viewport);
            self.viewport.center_mercator = [
                anchor[0] - f64::from(pointer.x - rect.center().x) / scale,
                anchor[1] - f64::from(pointer.y - rect.center().y) / scale,
            ];
        }
        self.viewport.normalize();
        if self.viewport != before {
            self.sync_active_view();
        }
        if !pin_captured
            && response.secondary_clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.reap_pin_at(rect, pointer);
        } else if !pin_captured
            && response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let persistent = ui.input(|input| input.modifiers.shift);
            self.strike_point(rect, pointer, persistent);
            self.water.click(egui::Rect::from_center_size(
                pointer,
                egui::Vec2::splat(18.0),
            ));
        }
    }

    fn tug_pins(&mut self, ui: &egui::Ui, map_rect: egui::Rect) -> PinGesture {
        let mut gesture = PinGesture::default();
        let scale = map::world_pixels(self.viewport);
        let mut moved = false;
        let mut seized_any = false;
        for slot in 0..self.pins.len() {
            let pin = self.pins[slot];
            let anchor = map::screen_at(self.viewport, map_rect, pin.world());
            let response = ui.interact(
                forge::pin_grip(anchor),
                egui::Id::new(("pin-bulb", slot)),
                egui::Sense::drag(),
            );
            let seized = response.dragged_by(egui::PointerButton::Primary);
            seized_any |= seized;
            if response.drag_started_by(egui::PointerButton::Primary) {
                self.pin_tug = Some(PinTug {
                    slot,
                    origin: pin,
                    world_points: scale,
                });
            }
            gesture.captured |= response.contains_pointer() || seized || response.drag_stopped();
            if seized || response.hovered() {
                gesture.hot = Some(slot);
                ui.ctx().set_cursor_icon(if seized {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                });
            }
            if seized
                && let (Some(tug), Some(delta)) = (self.pin_tug, response.total_drag_delta())
                && tug.slot == slot
            {
                let displaced = tug.origin.shifted([
                    f64::from(delta.x) / tug.world_points,
                    f64::from(delta.y) / tug.world_points,
                ]);
                moved |= displaced != self.pins[slot];
                self.pins[slot] = displaced;
            }
        }
        if !seized_any {
            self.pin_tug = None;
        }
        if moved {
            self.sync_active_view();
        }
        gesture
    }

    fn strike_point(&mut self, rect: egui::Rect, pointer: egui::Pos2, persistent: bool) {
        let Some(point) = MercatorPoint::forge(map::world_at(self.viewport, rect, pointer)) else {
            return;
        };
        if persistent {
            self.pins.push(point);
            self.sync_active_view();
        } else {
            self.transient_probe = Some(point);
        }
    }

    fn reap_pin_at(&mut self, map_rect: egui::Rect, pointer: egui::Pos2) {
        let victim = self
            .pins
            .iter()
            .enumerate()
            .map(|(slot, pin)| {
                let anchor = map::screen_at(self.viewport, map_rect, (*pin).world());
                (slot, anchor.distance_sq(pointer))
            })
            .filter(|(_, distance)| *distance <= 12.0_f32.powi(2))
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .map(|(slot, _)| slot);
        if let Some(victim) = victim {
            let _reaped = self.pins.remove(victim);
            self.sync_active_view();
        }
    }

    fn show_marks(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        map_rect: egui::Rect,
        hot_pin: Option<usize>,
    ) {
        // Probe chrome follows the retained field so the overlay, timestamp,
        // and sample advance atomically when a forecast finishes loading.
        let field = self.active_field().or_else(|| self.displayed_field.clone());
        let key = field
            .as_ref()
            .map(|(key, _)| *key)
            .or_else(|| self.active_key());

        if let Some(probe) = self.transient_probe {
            let anchor = map::screen_at(self.viewport, map_rect, probe.world());
            if map_rect.expand(8.0).contains(anchor) {
                let _dot = painter.circle_filled(anchor, 3.25, Color32::from_rgb(45, 42, 37));
                let _rim = painter.circle_stroke(
                    anchor,
                    3.25,
                    egui::Stroke::new(1.0_f32, chrome::SURFACE),
                );
                let _closed = self.point_popup(
                    ctx,
                    egui::Id::new("transient-probe"),
                    anchor + egui::vec2(9.0, 9.0),
                    probe,
                    key,
                    field.as_ref(),
                    false,
                );
            }
        }

        let mut victims = Vec::new();
        for (slot, pin) in self.pins.iter().copied().enumerate() {
            let anchor = map::screen_at(self.viewport, map_rect, pin.world());
            if !map_rect.expand(8.0).contains(anchor) {
                continue;
            }
            let crown = forge::pin_bulb(anchor);
            forge::pin(painter, anchor, hot_pin == Some(slot));
            if self.point_popup(
                ctx,
                egui::Id::new(("persistent-pin", slot)),
                crown + egui::vec2(11.0, 7.0),
                pin,
                key,
                field.as_ref(),
                true,
            ) {
                victims.push(slot);
            }
        }
        if !victims.is_empty() {
            for victim in victims.into_iter().rev() {
                let _reaped = self.pins.remove(victim);
            }
            self.sync_active_view();
        }
    }

    fn point_popup(
        &self,
        ctx: &egui::Context,
        id: egui::Id,
        position: egui::Pos2,
        point: MercatorPoint,
        key: Option<FrameKey>,
        field: Option<&(FrameKey, Arc<FieldGrid>)>,
        removable: bool,
    ) -> bool {
        let sample = field.and_then(|(_, field)| sample_point(point, field));
        let mut reap = false;
        let _popup = egui::Area::new(id)
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .show(ctx, |ui| {
                let _frame = egui::Frame::new()
                    .fill(chrome::SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, chrome::EDGE_STRONG))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        let valid = self.run.map_or_else(
                            || "AWAITING CYCLE".to_owned(),
                            |run| {
                                run.valid_local_label(self.slate.lead)
                                    .unwrap_or_else(|_| "INVALID TIME".to_owned())
                            },
                        );
                        let _head = ui.horizontal(|ui| {
                            let _time = ui.label(chrome::eyebrow(valid));
                            if removable {
                                reap = map_icon(ui, "×").on_hover_text("remove pin").clicked();
                            }
                        });
                        if let (Some(key), Some(raw)) = (key, sample) {
                            let scale = self.scale_for(key);
                            let _value = ui.label(chrome::section_title(scale.display(raw)));
                        } else if self.slate.overlay.active().is_some() {
                            let _pending = ui.label(chrome::muted(if field.is_some() {
                                "OUTSIDE HRRR DOMAIN"
                            } else {
                                "UPDATING…"
                            }));
                        }
                        let [longitude, latitude] = map::lon_lat_at(point.world());
                        let _position =
                            ui.label(chrome::muted(format!("{latitude:.4}°, {longitude:.4}°")));
                    });
            });
        reap
    }

    fn legend(painter: &egui::Painter, rect: egui::Rect, scale: &Scale) {
        const FONT_SIZE: f32 = 20.0;
        const WIDTH: f32 = 360.0;

        let width = (rect.width() - 32.0).clamp(1.0, WIDTH);
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.right() - width - 16.0, rect.bottom() - 31.0),
            egui::vec2(width, 12.0),
        );
        let count = scale.bins.len().saturating_sub(1);
        for (slot, pair) in scale.bins.windows(2).enumerate() {
            let left = bar.left() + bar.width() * slot as f32 / count as f32;
            let right = bar.left() + bar.width() * (slot + 1) as f32 / count as f32;
            let color = Color32::from_rgba_unmultiplied(
                pair[1].srgb[0],
                pair[1].srgb[1],
                pair[1].srgb[2],
                235,
            );
            let _stop = painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(left, bar.top()),
                    egui::pos2(right, bar.bottom()),
                ),
                0.0,
                color,
            );
        }
        let _edge = painter.rect_stroke(
            bar,
            0.0,
            egui::Stroke::new(1.0_f32, Color32::from_rgb(35, 31, 26)),
            egui::StrokeKind::Inside,
        );
        let font = egui::FontId::monospace(FONT_SIZE);
        let minimum = format!("{:.1}", scale.bins[0].ceiling);
        Self::legend_text(
            painter,
            bar.left_top() - egui::vec2(0.0, 3.0),
            egui::Align2::LEFT_BOTTOM,
            &minimum,
            font.clone(),
            Color32::from_black_alpha(240),
        );
        if let Some(last) = scale.bins.last() {
            let maximum = format!("{:.0} {}", last.ceiling, scale.unit.symbol());
            Self::legend_text(
                painter,
                bar.right_top() - egui::vec2(0.0, 3.0),
                egui::Align2::RIGHT_BOTTOM,
                &maximum,
                font,
                Color32::from_black_alpha(240),
            );
        }
    }

    fn legend_text(
        painter: &egui::Painter,
        anchor: egui::Pos2,
        align: egui::Align2,
        text: &str,
        font: egui::FontId,
        ink: Color32,
    ) {
        let paper = Color32::from_rgba_unmultiplied(
            basemap::PAPER_SRGB[0],
            basemap::PAPER_SRGB[1],
            basemap::PAPER_SRGB[2],
            235,
        );
        for offset in [
            egui::vec2(-1.5, 0.0),
            egui::vec2(1.5, 0.0),
            egui::vec2(0.0, -1.5),
            egui::vec2(0.0, 1.5),
            egui::vec2(-1.1, -1.1),
            egui::vec2(1.1, -1.1),
            egui::vec2(-1.1, 1.1),
            egui::vec2(1.1, 1.1),
        ] {
            let _halo = painter.text(anchor + offset, align, text, font.clone(), paper);
        }
        let _ink = painter.text(anchor, align, text, font, ink);
    }

    fn absorb_events(&mut self, _ctx: &egui::Context) {
        while let Ok(message) = self.custodian.faults.try_recv() {
            self.status = message;
        }
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Discovered(extent) => {
                    let latest = extent.run();
                    let prior_latest = self.latest_run.replace(latest);
                    let _prior_extent = self.run_extents.insert(latest, extent.published());
                    let selection = self.slate.cycle.rectify(latest);
                    if selection != self.slate.cycle {
                        self.slate.cycle = selection;
                        self.mark_dirty();
                    }
                    let run = selection.bind(latest);
                    let run_changed = self.run != Some(run);
                    if run_changed {
                        self.rebase_forecast(run);
                        self.mark_dirty();
                    }
                    if self.announced_discovery || prior_latest != Some(latest) {
                        "cycle index ready".clone_into(&mut self.status);
                    }
                    self.announced_discovery = false;
                    self.next_survey = Instant::now() + FRONTIER_POLL;
                    if run == latest || self.run_extents.contains_key(&run) {
                        self.reconcile_forecast();
                    } else {
                        self.request_survey(run);
                    }
                }
                Event::Surveyed(extent) => {
                    let run = extent.run();
                    if self.surveying_run == Some(run) {
                        self.surveying_run = None;
                    }
                    let prior = self.run_extents.insert(run, extent.published());
                    self.next_survey = Instant::now() + FRONTIER_POLL;
                    if self.run == Some(run) {
                        self.clamp_lead();
                        let active = self.active_key();
                        if active.is_some()
                            && self
                                .displayed_field
                                .as_ref()
                                .is_none_or(|(key, _field)| Some(*key) != active)
                        {
                            self.demand_active();
                        } else if prior != Some(extent.published()) {
                            self.status = format!("{} publication frontier", extent.published());
                        }
                    }
                }
                Event::SurveyFault { run, message } => {
                    if self.surveying_run == Some(run) {
                        self.surveying_run = None;
                    }
                    self.next_survey = Instant::now() + FRONTIER_POLL;
                    self.status = message;
                }
                Event::Loaded {
                    demand,
                    field,
                    elapsed_ms,
                } => {
                    if self.loading == Some(demand) {
                        self.loading = None;
                    }
                    let foreground = demand.intent == LoadIntent::Foreground(self.demand_id)
                        && self.active_key() == Some(demand.key);
                    if foreground {
                        self.displayed_field = Some((demand.key, field.clone()));
                    }
                    self.fields.insert(demand.key, field);
                    if foreground {
                        self.status = format!("{} decoded in {elapsed_ms} ms", demand.key.lead);
                    }
                    self.kick_prefetch();
                }
                Event::Fault { demand, message } => {
                    if let Some(demand) = demand {
                        if demand.intent == LoadIntent::Foreground(self.demand_id)
                            && self.active_key() == Some(demand.key)
                        {
                            self.status = message;
                        }
                        if self.loading == Some(demand) {
                            self.loading = None;
                        }
                        self.kick_prefetch();
                    } else {
                        self.announced_discovery = false;
                        self.status = format!("live refresh failed · {message}");
                    }
                }
            }
        }
        while let Ok(event) = self.basemap.events.try_recv() {
            match event {
                basemap::Event::Ready => {
                    "PROTOMAPS · © OPENSTREETMAP".clone_into(&mut self.basemap_status);
                }
                basemap::Event::Loaded(tile) => {
                    let key = tile.key;
                    let _was_inflight = self.tile_inflight.remove(&key);
                    self.basemap_status = format!(
                        "PROTOMAPS · OSM · {} KB · {} µs MAP + {} µs CUT",
                        tile.timing.bytes / 1024,
                        tile.timing.archive_us,
                        tile.timing.decode_us
                    );
                    self.tiles.insert(tile);
                }
                basemap::Event::Missing(key) => {
                    let _was_inflight = self.tile_inflight.remove(&key);
                    let _fresh = self.tile_faults.insert(key);
                }
                basemap::Event::Fault { key, message } => {
                    if let Some(key) = key {
                        let _was_inflight = self.tile_inflight.remove(&key);
                        let _fresh = self.tile_faults.insert(key);
                    }
                    self.basemap_status = format!("BASEMAP UNAVAILABLE · {message}");
                }
            }
        }
    }

    fn tend_survey(&mut self, ctx: &egui::Context) {
        let Some(run) = self.run else {
            return;
        };
        let incomplete = self
            .run_extents
            .get(&run)
            .is_some_and(|published| run.horizon().is_ok_and(|horizon| *published < horizon));
        if !incomplete {
            return;
        }
        let now = Instant::now();
        if now >= self.next_survey && self.surveying_run.is_none() && self.loading.is_none() {
            self.request_survey(run);
        } else if self.surveying_run.is_none() {
            let delay = if now >= self.next_survey {
                Duration::from_secs(1)
            } else {
                self.next_survey.duration_since(now)
            };
            ctx.request_repaint_after(delay);
        }
    }

    fn request_survey(&mut self, run: RunId) {
        if self.surveying_run == Some(run) {
            return;
        }
        match self.worker.send(Command::Survey(run)) {
            Ok(()) => {
                self.surveying_run = Some(run);
                self.next_survey = Instant::now() + FRONTIER_POLL;
                "surveying HRRR publication frontier…".clone_into(&mut self.status);
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn request_discovery(&mut self) {
        match self.worker.send(Command::Discover) {
            Ok(()) => {
                self.announced_discovery = true;
                "finding the newest complete HRRR cycle…".clone_into(&mut self.status);
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn take_keys(&mut self, ctx: &egui::Context) {
        let latest_long = ctx.input_mut(|input| input.consume_shortcut(&LATEST_LONG_RUN));
        if latest_long {
            self.follow_cycle(RunSelection::LatestLong);
        } else if ctx.input_mut(|input| input.consume_shortcut(&LATEST_RUN)) {
            self.follow_cycle(RunSelection::Latest);
        }
        if !ctx.text_edit_focused() {
            while let Some((slot, assign)) = consume_view_slot(ctx) {
                if assign {
                    self.assign_view_slot(slot);
                } else {
                    self.load_view_slot(slot);
                }
            }
        }
        if !ctx.text_edit_focused()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.transient_probe = None;
        }
        let frontier = self.run.and_then(|run| self.run_extents.get(&run)).copied();
        if frontier.is_some()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft))
        {
            self.choose_lead(self.slate.lead.saturating_previous());
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight))
            && let Some(frontier) = frontier
        {
            self.choose_lead(self.slate.lead.saturating_next(frontier));
        }
    }

    fn apply_view_actions(&mut self, actions: Vec<ViewAction<SavedView>>) {
        for action in actions {
            match action {
                ViewAction::New => self.new_view(),
                ViewAction::BeginNameEdit => self.begin_view_name_edit(),
                ViewAction::Rename => self.rename_view(),
                ViewAction::Load(view) => self.load_view(view),
                ViewAction::Clone(name) => self.clone_view(&name),
                ViewAction::Delete(name) => self.delete_view(&name),
                ViewAction::Moor { name, berth } => {
                    self.views.moor(&name, &berth);
                    self.mark_views_dirty();
                }
                ViewAction::NewShelf => {
                    self.views.add_shelf();
                    self.mark_views_dirty();
                }
                ViewAction::ToggleShelf(shelf) => {
                    self.views.toggle_shelf(shelf);
                    self.slate.closed_folders = self.views.closed_folders();
                    self.mark_dirty();
                }
                ViewAction::ScuttleShelf(shelf) => {
                    self.views.scuttle_shelf(shelf);
                    self.shelf_edit = None;
                    self.slate.closed_folders = self.views.closed_folders();
                    self.mark_dirty();
                    self.mark_views_dirty();
                }
                ViewAction::BeginShelfRename(shelf) => {
                    let name = self
                        .views
                        .shelves
                        .get(shelf)
                        .map(|rack| rack.name.clone())
                        .unwrap_or_default();
                    self.shelf_edit = Some(ShelfEdit {
                        shelf,
                        name,
                        focus: true,
                    });
                }
                ViewAction::CommitShelfRename => {
                    if let Some(edit) = self.shelf_edit.take() {
                        if self.views.rename_shelf(edit.shelf, &edit.name) {
                            self.slate.closed_folders = self.views.closed_folders();
                            self.mark_dirty();
                            self.mark_views_dirty();
                        } else {
                            self.status = format!("folder `{}` already exists", edit.name.trim());
                        }
                    }
                }
            }
        }
    }

    fn sync_active_view(&mut self) {
        if let Some(view) = self.views.get_mut(&self.active_view) {
            view.reframe(self.viewport, self.pins.clone());
        } else {
            self.views.upsert(SavedView::forge(
                self.active_view.clone(),
                self.viewport,
                self.pins.clone(),
            ));
        }
        self.slate.active_view = Some(self.active_view.clone());
        self.mark_dirty();
        self.mark_views_dirty();
    }

    fn new_view(&mut self) {
        self.sync_active_view();
        let source = self.active_view.clone();
        let name = self.views.spare_named(&source);
        let view = SavedView::forge(name.clone(), self.viewport, self.pins.clone());
        self.views.adopt_beside(&source, view);
        self.active_view = name.clone();
        self.slate.active_view = Some(name.clone());
        self.view_name_entry.clear();
        self.name_edit = NameEdit::Idle;
        self.status = format!("new view `{name}`");
        self.mark_dirty();
        self.mark_views_dirty();
    }

    fn load_view(&mut self, view: SavedView) {
        self.active_view.clone_from(&view.name);
        self.slate.active_view = Some(view.name.clone());
        self.viewport = view.viewport;
        self.pins = view.pins;
        self.view_name_entry.clear();
        self.name_edit = NameEdit::Idle;
        self.status = format!("active view `{}`", view.name);
        self.mark_dirty();
    }

    fn load_view_slot(&mut self, slot: ViewSlot) {
        let view = self
            .views
            .all()
            .find(|view| view.slot == Some(slot))
            .cloned();
        if let Some(view) = view {
            self.load_view(view);
        } else {
            self.status = format!("view slot {slot} is unbound");
        }
    }

    fn assign_view_slot(&mut self, slot: ViewSlot) {
        self.sync_active_view();
        let mut displaced = None;
        for view in self.views.all_mut() {
            if view.slot == Some(slot) {
                displaced = Some(view.name.clone());
                view.slot = None;
            }
        }
        if let Some(view) = self.views.get_mut(&self.active_view) {
            view.slot = Some(slot);
        }
        self.status = displaced
            .filter(|name| name != &self.active_view)
            .map_or_else(
                || format!("bound view `{}` to {slot}", self.active_view),
                |name| format!("moved slot {slot} from `{name}` to `{}`", self.active_view),
            );
        self.mark_views_dirty();
    }

    fn clone_view(&mut self, name: &EntryName) {
        let Some(source) = self.views.get(name).cloned() else {
            return;
        };
        let clone_name = self.views.spare_named(name);
        let clone = SavedView::forge(clone_name.clone(), source.viewport, source.pins.clone());
        self.views.adopt_beside(name, clone.clone());
        self.load_view(clone);
        self.status = format!("cloned view `{clone_name}`");
        self.mark_views_dirty();
    }

    fn delete_view(&mut self, name: &EntryName) {
        if self.views.all().count() == 1 {
            "the last view cannot be deleted".clone_into(&mut self.status);
            return;
        }
        let Some(removed) = self.views.remove(name) else {
            return;
        };
        let successor = (self.active_view == removed.name)
            .then(|| self.views.all().next().cloned())
            .flatten();
        if let Some(next) = successor {
            self.load_view(next);
        }
        self.status = format!("deleted view `{}`", removed.name);
        self.mark_views_dirty();
    }

    fn begin_view_name_edit(&mut self) {
        self.view_name_entry = self.active_view.to_string();
        self.name_edit = NameEdit::Arming;
    }

    fn rename_view(&mut self) {
        let Some(new) = EntryName::forge(&self.view_name_entry) else {
            "rename needs a nonempty view name".clone_into(&mut self.status);
            return;
        };
        let old = self.active_view.clone();
        if old == new {
            self.view_name_entry.clear();
            self.name_edit = NameEdit::Idle;
            return;
        }
        if self.views.taken(&new) {
            self.status = format!("view `{new}` already exists");
            return;
        }
        self.sync_active_view();
        self.views.rename(&old, new.clone());
        self.active_view = new.clone();
        self.slate.active_view = Some(new.clone());
        self.view_name_entry.clear();
        self.name_edit = NameEdit::Idle;
        self.status = format!("renamed view `{old}` → `{new}`");
        self.mark_dirty();
        self.mark_views_dirty();
    }

    fn strike_overlay(&mut self, product: Product) {
        self.slate.overlay = self.slate.overlay.strike(product);
        self.mark_dirty();
        if self.slate.overlay.active().is_some() {
            self.demand_active();
        } else {
            self.demand_id.advance();
            self.loading = None;
            self.displayed_field = None;
            self.prefetch.clear();
            "basemap only".clone_into(&mut self.status);
        }
    }

    fn choose_lead(&mut self, lead: LeadHour) {
        let Some(frontier) = self.run.and_then(|run| self.run_extents.get(&run)).copied() else {
            return;
        };
        if lead <= frontier && self.slate.lead != lead {
            self.slate.lead = lead;
            self.mark_dirty();
            self.demand_active();
        }
    }

    fn follow_cycle(&mut self, selection: RunSelection) {
        if self.slate.cycle != selection {
            self.slate.cycle = selection;
            self.mark_dirty();
        }
        if let Some(latest) = self.latest_run {
            let run = selection.rectify(latest).bind(latest);
            if self.run != Some(run) {
                self.land_run(run);
            }
        }
        self.request_discovery();
    }

    fn land_run(&mut self, run: RunId) {
        if self.run == Some(run) {
            self.request_survey(run);
        } else {
            self.rebase_forecast(run);
            self.mark_dirty();
            if self.run_extents.contains_key(&run) {
                self.clamp_lead();
                self.demand_active();
            } else {
                self.demand_id.advance();
                self.prefetch.clear();
                self.request_survey(run);
            }
        }
    }

    fn reconcile_forecast(&mut self) {
        self.clamp_lead();
        let active = self.active_key();
        if active.is_some()
            && self
                .displayed_field
                .as_ref()
                .is_none_or(|(key, _field)| Some(*key) != active)
        {
            self.demand_active();
        }
    }

    fn clamp_lead(&mut self) {
        let Some(run) = self.run else {
            return;
        };
        let Some(published) = self.run_extents.get(&run).copied() else {
            return;
        };
        if self.slate.lead > published {
            self.slate.lead = published;
            self.mark_dirty();
        }
    }

    fn rebase_forecast(&mut self, run: RunId) {
        let frontier = self
            .run_extents
            .get(&run)
            .copied()
            .or_else(|| run.horizon().ok())
            .unwrap_or(LeadHour::ZERO);
        let lead = self.run.map_or(LeadHour::ZERO, |source| {
            run.rebase_lead(source, self.slate.lead, frontier)
        });
        self.run = Some(run);
        self.slate.lead = lead;
    }

    fn active_key(&self) -> Option<FrameKey> {
        let run = self.run?;
        let published = self.run_extents.get(&run)?;
        let product = self.slate.overlay.active()?;
        (self.slate.lead <= *published).then_some(FrameKey {
            run,
            lead: self.slate.lead,
            product,
        })
    }

    fn scale_for(&self, key: FrameKey) -> &Scale {
        self.scales
            .get(key.product, self.smoke_regime, TemperatureSeason::at(key))
    }

    fn demand_cover(&mut self, cover: &basemap::Cover) {
        if let Some(fallback) = cover.strata.first() {
            for &key in &fallback.keys {
                self.demand_tile(key);
            }
        }
        for stratum in &cover.strata {
            if stratum.intent.demands() {
                for &key in &stratum.keys {
                    self.demand_tile(key);
                }
            }
        }
        for stratum in cover.strata.iter().rev() {
            if stratum.intent == basemap::Intent::Retained {
                for &key in &stratum.keys {
                    self.demand_tile(key);
                }
            }
        }
    }

    fn demand_tile(&mut self, key: TileKey) {
        if !self.tiles.contains(key)
            && !self.tile_inflight.contains(&key)
            && !self.tile_faults.contains(&key)
            && self.basemap.request(key)
        {
            let _fresh = self.tile_inflight.insert(key);
        }
    }

    fn paint_labels(&self, painter: &egui::Painter, map_rect: egui::Rect) {
        let mut candidates = self
            .presented_basemap
            .iter()
            .flat_map(|tile| tile.labels.iter())
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|label| label.rank);
        let mut occupied = Vec::<egui::Rect>::new();
        for label in candidates {
            let maturity = basemap::apparition(self.viewport.zoom as f32, label.onset_zoom);
            if maturity <= 0.01 {
                continue;
            }
            let anchor = map::screen_at(self.viewport, map_rect, label.world);
            let size = label.size * (0.88 + 0.12 * maturity);
            let width = label.text.chars().count() as f32 * size * 0.58;
            let footprint =
                egui::Rect::from_center_size(anchor, egui::vec2(width.max(size), size * 1.25))
                    .expand(2.0);
            if !map_rect.contains_rect(footprint)
                || occupied.iter().any(|prior| prior.intersects(footprint))
            {
                continue;
            }
            occupied.push(footprint);
            if occupied.len() >= 180 {
                break;
            }
            let font = egui::FontId::proportional(size);
            let halo = Color32::from_rgba_unmultiplied(
                basemap::PAPER_SRGB[0],
                basemap::PAPER_SRGB[1],
                basemap::PAPER_SRGB[2],
                (178.0 * maturity) as u8,
            );
            for offset in [
                egui::vec2(-1.0, 0.0),
                egui::vec2(1.0, 0.0),
                egui::vec2(0.0, -1.0),
                egui::vec2(0.0, 1.0),
                egui::vec2(-0.7, -0.7),
                egui::vec2(0.7, -0.7),
                egui::vec2(-0.7, 0.7),
                egui::vec2(0.7, 0.7),
            ] {
                let _halo = painter.text(
                    anchor + offset,
                    egui::Align2::CENTER_CENTER,
                    label.text.as_ref(),
                    font.clone(),
                    halo,
                );
            }
            let _label = painter.text(
                anchor,
                egui::Align2::CENTER_CENTER,
                label.text.as_ref(),
                font,
                Color32::from_black_alpha((215.0 * maturity) as u8),
            );
        }
    }

    fn active_field(&self) -> Option<(FrameKey, Arc<FieldGrid>)> {
        let key = self.active_key()?;
        self.fields.get(key).map(|field| (key, field.clone()))
    }

    fn demand_active(&mut self) {
        let Some(key) = self.active_key() else {
            return;
        };
        self.demand_id.advance();
        self.prefetch.clear();
        self.seed_prefetch(key);
        if self.fields.get(key).is_some() {
            self.loading = None;
            self.displayed_field = self.fields.get(key).map(|field| (key, field.clone()));
            self.status = format!("{} ready", key.lead);
            self.kick_prefetch();
        } else {
            let demand = LoadDemand {
                intent: LoadIntent::Foreground(self.demand_id),
                key,
            };
            self.loading = Some(demand);
            self.status = format!("cutting {}…", key.lead);
            if let Err(err) = self.worker.send(Command::Load(demand)) {
                self.loading = None;
                self.status = err.to_string();
            }
        }
    }

    fn seed_prefetch(&mut self, key: FrameKey) {
        let horizon = self
            .run_extents
            .get(&key.run)
            .copied()
            .unwrap_or(LeadHour::ZERO);
        for distance in 1..=2 {
            if let Ok(lead) = LeadHour::forge(key.lead.get().saturating_add(distance))
                && lead <= horizon
            {
                self.prefetch.push_back(FrameKey { lead, ..key });
            }
            if key.lead.get() >= distance
                && let Ok(lead) = LeadHour::forge(key.lead.get() - distance)
            {
                self.prefetch.push_back(FrameKey { lead, ..key });
            }
        }
    }

    fn kick_prefetch(&mut self) {
        if self.loading.is_some() {
            return;
        }
        while let Some(key) = self.prefetch.pop_front() {
            if self.fields.get(key).is_some() {
                continue;
            }
            let demand = LoadDemand {
                intent: LoadIntent::Prefetch,
                key,
            };
            if self.worker.send(Command::Load(demand)).is_ok() {
                self.loading = Some(demand);
            }
            break;
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty = Some(Instant::now());
    }

    fn mark_config_dirty(&mut self) {
        self.config_dirty = Some(Instant::now());
    }

    fn mark_views_dirty(&mut self) {
        self.views_dirty = Some(Instant::now());
    }

    fn flush_state(&mut self, force: bool) {
        let ready = self
            .dirty
            .is_some_and(|dirty| force || dirty.elapsed() >= STATE_SETTLE);
        if !ready {
            return;
        }
        match self.slate.save(&self.lair.slate_path()) {
            Ok(()) => self.dirty = None,
            Err(err) => self.status = format!("state save failed: {err:#}"),
        }
    }

    fn flush_config(&mut self, force: bool) {
        let ready = self
            .config_dirty
            .is_some_and(|dirty| force || dirty.elapsed() >= STATE_SETTLE);
        if !ready {
            return;
        }
        match self.config.save(&self.lair.config_path()) {
            Ok(()) => self.config_dirty = None,
            Err(err) => self.status = format!("config save failed: {err:#}"),
        }
    }

    fn flush_views(&mut self, force: bool) {
        let ready = self
            .views_dirty
            .is_some_and(|dirty| force || dirty.elapsed() >= STATE_SETTLE);
        if !ready {
            return;
        }
        match self.views.save(&self.lair.views_path()) {
            Ok(()) => self.views_dirty = None,
            Err(err) => self.status = format!("view save failed: {err:#}"),
        }
    }
}

impl Drop for WeatherApp {
    fn drop(&mut self) {
        self.flush_state(true);
        self.flush_config(true);
        self.flush_views(true);
    }
}

fn consume_view_slot(ctx: &egui::Context) -> Option<(ViewSlot, bool)> {
    ctx.input_mut(|input| {
        let (index, command) =
            input.events.iter().enumerate().find_map(|(index, event)| {
                view_slot_command(event).map(|command| (index, command))
            })?;
        let _event = input.events.remove(index);
        Some(command)
    })
}

fn view_slot_command(event: &egui::Event) -> Option<(ViewSlot, bool)> {
    let egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        repeat: false,
        modifiers,
    } = event
    else {
        return None;
    };
    if modifiers.alt || modifiers.ctrl || modifiers.command || modifiers.mac_cmd {
        return None;
    }
    let digit = match physical_key.unwrap_or(*key) {
        egui::Key::Num0 => 0,
        egui::Key::Num1 => 1,
        egui::Key::Num2 => 2,
        egui::Key::Num3 => 3,
        egui::Key::Num4 => 4,
        egui::Key::Num5 => 5,
        egui::Key::Num6 => 6,
        egui::Key::Num7 => 7,
        egui::Key::Num8 => 8,
        egui::Key::Num9 => 9,
        _ => return None,
    };
    ViewSlot::forge(digit).map(|slot| (slot, modifiers.shift))
}

fn map_icon(ui: &mut egui::Ui, glyph: &str) -> egui::Response {
    let response = ui.add(chrome::icon_button(glyph).sense(egui::Sense::CLICK));
    chrome::tension(ui, &response);
    response
}

fn sample_point(point: MercatorPoint, field: &FieldGrid) -> Option<f32> {
    let [i, j] = map::grid_at(field, point.world()).map(f64::round);
    if !(0.0..f64::from(field.width)).contains(&i) || !(0.0..f64::from(field.height)).contains(&j) {
        return None;
    }
    field
        .at(i as u32, j as u32)
        .filter(|value| value.is_finite())
}

struct FrameBank {
    capacity: usize,
    fields: HashMap<FrameKey, Arc<FieldGrid>>,
    order: VecDeque<FrameKey>,
}

impl FrameBank {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            fields: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: FrameKey) -> Option<&Arc<FieldGrid>> {
        self.fields.get(&key)
    }

    fn insert(&mut self, key: FrameKey, field: Arc<FieldGrid>) {
        if self.fields.insert(key, field).is_none() {
            self.order.push_back(key);
        }
        while self.fields.len() > self.capacity {
            if let Some(victim) = self.order.pop_front() {
                let _evicted = self.fields.remove(&victim);
            }
        }
    }
}

struct VectorBank {
    ceiling: usize,
    bytes: usize,
    epoch: u64,
    tiles: HashMap<TileKey, VectorEntry>,
}

struct VectorEntry {
    tile: Arc<VectorTile>,
    bytes: usize,
    touched: u64,
}

impl VectorBank {
    fn new(ceiling: usize) -> Self {
        Self {
            ceiling,
            bytes: 0,
            epoch: 0,
            tiles: HashMap::new(),
        }
    }

    fn get(&mut self, key: TileKey) -> Option<&Arc<VectorTile>> {
        self.epoch = self.epoch.saturating_add(1);
        let entry = self.tiles.get_mut(&key)?;
        entry.touched = self.epoch;
        Some(&entry.tile)
    }

    fn contains(&self, key: TileKey) -> bool {
        self.tiles.contains_key(&key)
    }

    fn insert(&mut self, tile: Arc<VectorTile>) {
        let key = tile.key;
        let bytes = tile.resident_bytes();
        self.epoch = self.epoch.saturating_add(1);
        let fresh = VectorEntry {
            tile,
            bytes,
            touched: self.epoch,
        };
        if let Some(prior) = self.tiles.insert(key, fresh) {
            self.bytes = self.bytes.saturating_sub(prior.bytes);
        }
        self.bytes = self.bytes.saturating_add(bytes);
        while self.bytes > self.ceiling && self.tiles.len() > 1 {
            let victim = self
                .tiles
                .iter()
                .min_by_key(|(_key, entry)| entry.touched)
                .map(|(key, _entry)| *key);
            let Some(victim) = victim else { break };
            let Some(victim) = self.tiles.remove(&victim) else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(victim.bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_digits_survive_shifted_logical_keys() {
        let shifted = egui::Event::Key {
            key: egui::Key::Exclamationmark,
            physical_key: Some(egui::Key::Num1),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        let command = view_slot_command(&shifted).map(|(slot, assign)| (slot.digit(), assign));
        assert_eq!(command, Some((1, true)));

        let plain = egui::Event::Key {
            key: egui::Key::Num7,
            physical_key: Some(egui::Key::Num7),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let command = view_slot_command(&plain).map(|(slot, assign)| (slot.digit(), assign));
        assert_eq!(command, Some((7, false)));
    }
}
