use crate::{
    basemap::{self, Basemap, TileKey, VectorTile},
    cache::Custodian,
    config::{Config, ConfigLoad},
    decrees::{self, Decree},
    library::EntryName,
    library_ui::{self, Action as ViewAction, EntryEdit, NameEdit, ShelfEdit},
    map::{self, FieldPaint},
    model::{FieldGrid, FrameKey, LeadHour, MercatorPoint, Product, RunId, RunSelection, Viewport},
    spec::{Scale, ScaleAtlas, SmokeRegime, TemperatureSeason},
    state::Slate,
    vector_map::VectorPaint,
    view::{SavedView, ViewLibrary, ViewSlot},
    wind_barb,
    worker::{Command, DemandId, Event, LoadDemand, LoadIntent, Worker},
    xdg::{InstanceGuard, Lair},
};
use anyhow::Result;
use brass_poolrooms::{
    chrome,
    water::{Domain, Frame as WaterFrame, Surface, Wetness},
};
use egui::Color32;
use eternalist_apps::{
    ScribeOutcome, SettledScribe,
    command_guide::{CommandGuide, GuideGesture, GuideSection, PANEL_IDIOMS, RAIL_IDIOMS},
    commands::{CommandDispatch, CommandStatus, Shortcut, ShortcutKey, ShortcutModifiers},
    panel_navigation::PanelNavigator,
    responsiveness::DrainBudget,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

const STATE_SETTLE: Duration = Duration::from_millis(450);
const SCALE_SETTLE: Duration = Duration::from_millis(180);
const FRONTIER_POLL: Duration = Duration::from_mins(1);
const TILE_RETRY_DELAY: Duration = Duration::from_secs(15);
const EVENT_DRAIN: DrainBudget = DrainBudget::new(64, Duration::from_millis(3));
const FIELD_CAPACITY: usize = 12;
const VECTOR_CEILING: usize = 512 * 1_048_576;
const VIEW_SLOT_KEYS: [Shortcut; 10] = [
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('0')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('1')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('2')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('3')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('4')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('5')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('6')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('7')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('8')),
    Shortcut::new(ShortcutModifiers::NONE, ShortcutKey::Character('9')),
];
const ASSIGN_VIEW_SLOT_KEYS: [Shortcut; 10] = [
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('0')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('1')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('2')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('3')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('4')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('5')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('6')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('7')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('8')),
    Shortcut::new(ShortcutModifiers::SHIFT, ShortcutKey::Character('9')),
];
const VIEW_GESTURES: [GuideGesture; 2] = [
    GuideGesture::new(
        "Select bound view",
        "Loads the view assigned to one numeric berth.",
        &VIEW_SLOT_KEYS,
    ),
    GuideGesture::new(
        "Bind active view",
        "Assigns the current view to one numeric berth.",
        &ASSIGN_VIEW_SLOT_KEYS,
    ),
];
const VIEW_IDIOMS: GuideSection = GuideSection::new("VIEW BERTHS", &VIEW_GESTURES);
const GUIDE_SECTIONS: [GuideSection; 3] = [PANEL_IDIOMS, RAIL_IDIOMS, VIEW_IDIOMS];

#[derive(Clone, Copy)]
enum TileRejection {
    Absent,
    RetryAt(Instant),
}

impl TileRejection {
    fn blocks(self, now: Instant) -> bool {
        match self {
            Self::Absent => true,
            Self::RetryAt(deadline) => now < deadline,
        }
    }

    const fn resolves(self) -> bool {
        matches!(self, Self::Absent)
    }
}

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

struct ScaleLatch<S> {
    settled: S,
    contender: Option<(S, Instant)>,
}

impl<S: Default> Default for ScaleLatch<S> {
    fn default() -> Self {
        Self {
            settled: S::default(),
            contender: None,
        }
    }
}

impl<S: Copy + Eq> ScaleLatch<S> {
    const fn settled(&self) -> S {
        self.settled
    }

    fn observe(&mut self, proposed: S, now: Instant) -> Option<Duration> {
        if proposed == self.settled {
            self.contender = None;
            return None;
        }
        let Some((contender, born)) = self.contender else {
            self.contender = Some((proposed, now));
            return Some(SCALE_SETTLE);
        };
        if contender != proposed {
            self.contender = Some((proposed, now));
            return Some(SCALE_SETTLE);
        }
        let age = now.saturating_duration_since(born);
        if age >= SCALE_SETTLE {
            self.settled = proposed;
            self.contender = None;
            None
        } else {
            Some(SCALE_SETTLE.saturating_sub(age))
        }
    }

    fn arrest(&mut self) {
        self.contender = None;
    }
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

impl PinTug {
    fn reversal(self, present: &[MercatorPoint]) -> Option<Vec<MercatorPoint>> {
        (present.get(self.slot).copied()? != self.origin).then(|| {
            let mut before = present.to_vec();
            before[self.slot] = self.origin;
            before
        })
    }
}

enum MapReversal {
    Pins {
        view: EntryName,
        before: Vec<MercatorPoint>,
    },
    Probe {
        before: Option<MercatorPoint>,
    },
}

impl MapReversal {
    fn belongs_to(&self, view: &EntryName) -> bool {
        match self {
            Self::Pins { view: owner, .. } => owner == view,
            Self::Probe { .. } => true,
        }
    }

    fn recoil(self, pins: &[MercatorPoint], probe: Option<MercatorPoint>) -> Option<MapRecoil> {
        match self {
            Self::Pins { before, .. } if before != pins => Some(MapRecoil::Pins(before)),
            Self::Probe { before } if before != probe => Some(MapRecoil::Probe(before)),
            Self::Pins { .. } | Self::Probe { .. } => None,
        }
    }
}

#[derive(Debug, PartialEq)]
enum MapRecoil {
    Pins(Vec<MercatorPoint>),
    Probe(Option<MercatorPoint>),
}

#[derive(Default)]
struct MapUndo {
    reversals: VecDeque<MapReversal>,
}

impl MapUndo {
    const CAPACITY: usize = 64;

    fn remember(&mut self, reversal: MapReversal) {
        if self.reversals.len() == Self::CAPACITY {
            let _forgotten = self.reversals.pop_front();
        }
        self.reversals.push_back(reversal);
    }

    fn has_reversal_for(&self, view: &EntryName) -> bool {
        self.reversals
            .iter()
            .any(|reversal| reversal.belongs_to(view))
    }

    fn recoil(
        &mut self,
        view: &EntryName,
        pins: &[MercatorPoint],
        probe: Option<MercatorPoint>,
    ) -> Option<MapRecoil> {
        loop {
            let slot = self
                .reversals
                .iter()
                .rposition(|reversal| reversal.belongs_to(view))?;
            let reversal = self.reversals.remove(slot)?;
            if let Some(recoil) = reversal.recoil(pins, probe) {
                return Some(recoil);
            }
        }
    }

    fn rename_view(&mut self, old: &EntryName, new: &EntryName) {
        for reversal in &mut self.reversals {
            if let MapReversal::Pins { view, .. } = reversal
                && view == old
            {
                view.clone_from(new);
            }
        }
    }

    fn forget_view(&mut self, view: &EntryName) {
        self.reversals.retain(
            |reversal| !matches!(reversal, MapReversal::Pins { view: owner, .. } if owner == view),
        );
    }
}

#[derive(Clone, Copy)]
struct PlaqueBerth {
    position: egui::Pos2,
    pivot: egui::Align2,
}

impl PlaqueBerth {
    fn rect(self, size: egui::Vec2) -> egui::Rect {
        self.pivot.anchor_size(self.position, size)
    }
}

struct PlaquePhalanx {
    bounds: egui::Rect,
    occupied: Vec<egui::Rect>,
}

impl PlaquePhalanx {
    const CLEARANCE: f32 = 3.0;

    fn forge(bounds: egui::Rect) -> Self {
        Self {
            bounds,
            occupied: Vec::new(),
        }
    }

    fn berth(&self, anchor: egui::Pos2, size: egui::Vec2, gap: egui::Vec2) -> PlaqueBerth {
        let right = PlaqueBerth {
            position: anchor + gap,
            pivot: egui::Align2::LEFT_TOP,
        };
        let left = PlaqueBerth {
            position: anchor + egui::vec2(-gap.x, gap.y),
            pivot: egui::Align2::RIGHT_TOP,
        };
        let clear = |berth: PlaqueBerth| {
            let rect = berth.rect(size);
            !self
                .occupied
                .iter()
                .any(|prior| prior.intersects(rect.expand(Self::CLEARANCE)))
        };
        let contained = |berth: PlaqueBerth| self.bounds.contains_rect(berth.rect(size));

        [right, left]
            .into_iter()
            .find(|&berth| contained(berth) && clear(berth))
            .or_else(|| [right, left].into_iter().find(|&berth| clear(berth)))
            .unwrap_or(right)
    }

    fn occupy(&mut self, rect: egui::Rect) {
        self.occupied.push(rect);
    }
}

struct PlaqueResponse {
    reap: bool,
    rect: egui::Rect,
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

#[derive(Clone, Copy, Default)]
struct DirtyState {
    slate: bool,
    config: bool,
    views: bool,
}

impl DirtyState {
    const fn any(self) -> bool {
        self.slate || self.config || self.views
    }
}

struct DurableState {
    slate: Option<Slate>,
    config: Option<Config>,
    views: Option<ViewLibrary>,
}

impl DurableState {
    fn save(self, lair: &Lair) -> Result<()> {
        let mut faults = Vec::new();
        if let Some(slate) = self.slate
            && let Err(error) = slate.save(&lair.slate_path())
        {
            faults.push(format!("session state: {error:#}"));
        }
        if let Some(config) = self.config
            && let Err(error) = config.save(&lair.config_path())
        {
            faults.push(format!("preferences: {error:#}"));
        }
        if let Some(views) = self.views
            && let Err(error) = views.save(&lair.views_path())
        {
            faults.push(format!("saved views: {error:#}"));
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(faults.join("; ")))
        }
    }
}

pub struct WeatherApp {
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
    tile_rejections: HashMap<TileKey, TileRejection>,
    panels: PanelNavigator,
    guide: CommandGuide,
    transient_probe: Option<MercatorPoint>,
    pin_tug: Option<PinTug>,
    map_undo: MapUndo,
    view_name_entry: String,
    name_edit: NameEdit,
    shelf_edit: Option<ShelfEdit>,
    entry_edit: Option<EntryEdit>,
    scales: ScaleAtlas,
    smoke_scale: ScaleLatch<SmokeRegime>,
    smoke_survey: SmokeSurvey,
    scale_bar: map::ScaleBar,
    water: Surface,
    scribe: SettledScribe<DurableState>,
    dirty: DirtyState,
    status: String,
    basemap_status: String,
}

impl WeatherApp {
    pub fn open_at(ctx: &egui::Context, lair: Lair, instance: InstanceGuard) -> Result<Self> {
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
        let basemap = Basemap::spawn(ctx.clone(), &lair)?;
        let custodian = Custodian::spawn(ctx.clone(), lair.cache_manager())?;
        #[cfg(feature = "egui-test")]
        let run_extents = witnessed_frontier(run).into_iter().collect();
        #[cfg(not(feature = "egui-test"))]
        let run_extents = HashMap::new();
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
        let scribe_lair = lair.clone();
        let mut scribe = SettledScribe::spawn(
            "hrrr-state-scribe",
            ctx,
            STATE_SETTLE,
            move |state: DurableState| state.save(&scribe_lair),
        )?;
        let dirty = DirtyState {
            slate: migrated_slate,
            config: had_legacy_views,
            views: migrated_views,
        };
        if dirty.any() {
            scribe.mark();
        }
        let app = Self {
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
            run_extents,
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
            tile_rejections: HashMap::new(),
            panels: PanelNavigator::default(),
            guide: CommandGuide::default(),
            transient_probe: None,
            pin_tug: None,
            map_undo: MapUndo::default(),
            view_name_entry: String::new(),
            name_edit: NameEdit::Idle,
            shelf_edit: None,
            entry_edit: None,
            scales: ScaleAtlas::default(),
            smoke_scale: ScaleLatch::default(),
            smoke_survey: SmokeSurvey::default(),
            scale_bar: map::ScaleBar::default(),
            water,
            scribe,
            dirty,
            status: "finding latest forecast…".to_owned(),
            basemap_status: "OPENING MAP".to_owned(),
        };
        app.worker.send(Command::Discover)?;
        Ok(app)
    }

    pub fn pulse(&mut self, ui: &mut egui::Ui) {
        self.absorb_events(ui.ctx());
        self.absorb_persistence();
        let guide_invoked = self.guide.take_shortcuts(ui.ctx());
        if !guide_invoked
            && let Some(dispatch) =
                decrees::canon().route(ui.ctx(), &[], |decree| self.decree_status(decree))
        {
            self.apply_decree(dispatch);
        }
        self.take_keys(ui.ctx());
        let mut panels = std::mem::take(&mut self.panels);
        let inspector = eternalist_apps::Inspector::new("forecast-inspector")
            .scroll_id("forecast-inspector-scroll")
            .scroll_offset(self.slate.inspector_scroll)
            .show(ui, |ui| self.inspector(ui, &mut panels));
        self.panels = panels;
        if inspector.scroll_offset != self.slate.inspector_scroll {
            self.slate.inspector_scroll = inspector.scroll_offset;
            self.mark_dirty();
        }
        self.water.heave(ui.ctx(), inspector.scroll_offset);
        let _center = egui::CentralPanel::default().show(ui, |ui| self.map(ui));
        let mut guide = std::mem::take(&mut self.guide);
        guide.show(
            ui.ctx(),
            decrees::canon(),
            &[],
            |()| "FORECAST",
            |decree| self.decree_status(decree),
            &GUIDE_SECTIONS,
        );
        if let Some(rect) = guide.rect() {
            crate::witness::rect(ui.ctx(), hrrr_contract::Target::CommandGuide, rect);
        }
        self.guide = guide;
    }

    pub fn service_deadline(&self, _now: Instant) -> Option<Instant> {
        self.scribe
            .deadline()
            .into_iter()
            .chain(self.survey_deadline())
            .chain(self.tile_rejections.values().filter_map(|rejection| {
                if let TileRejection::RetryAt(deadline) = rejection {
                    Some(*deadline)
                } else {
                    None
                }
            }))
            .min()
    }

    pub fn service_deadline_reached(&mut self, now: Instant) -> bool {
        let mut changed = false;
        self.tile_rejections.retain(|_key, rejection| {
            let expired = matches!(rejection, TileRejection::RetryAt(deadline) if *deadline <= now);
            changed |= expired;
            !expired
        });
        if self
            .survey_deadline()
            .is_some_and(|deadline| deadline <= now)
            && let Some(run) = self.run
        {
            self.request_survey(run);
            changed = true;
        }
        if self
            .scribe
            .deadline()
            .is_some_and(|deadline| deadline <= now)
        {
            let snapshot = self.durable_state();
            match self.scribe.tend(now, || snapshot) {
                Ok(Some(_sequence)) => self.dirty = DirtyState::default(),
                Ok(None) => {}
                Err(error) => {
                    self.status = format!("state scribe failed: {error:#}");
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn water_frame(
        &mut self,
        ctx: &egui::Context,
        pixels_per_point: f32,
        tooltip_rects: &[egui::Rect],
    ) -> WaterFrame {
        self.water.frame(ctx, pixels_per_point, tooltip_rects, None)
    }

    fn retire(&mut self) {
        if let Err(error) = self.scribe.flush(self.durable_state_all()) {
            eprintln!("could not persist HRRR state during retirement: {error:#}");
        }
        self.dirty = DirtyState::default();
    }

    pub const fn close_to_tray_enabled(&self) -> bool {
        self.config.close_minimizes
    }

    fn inspector(&mut self, ui: &mut egui::Ui, navigator: &mut PanelNavigator) {
        let mut panels = navigator.frame(ui.ctx());
        let mut chosen_product = None;
        let field = panels.section(ui, "product", "field", true, |ui| {
            for &row in Product::ROWS {
                let _row = ui.horizontal(|ui| {
                    let spacing = ui.spacing().item_spacing.x * row.len().saturating_sub(1) as f32;
                    let width = (ui.available_width() - spacing) / row.len() as f32;
                    for &product in row {
                        let response = ui.add_sized(
                            [width, 26.0],
                            egui::Button::new(product.label())
                                .selected(self.slate.overlay.active() == Some(product)),
                        );
                        crate::witness::anchor(
                            ui,
                            hrrr_contract::Target::Field(product.cache_name()),
                            response.rect,
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
        crate::witness::response(ui, hrrr_contract::Target::Panel("field"), &field.header);
        self.water.fold(field.wake);
        if let Some((product, rect)) = chosen_product {
            self.water.select(rect);
            self.strike_overlay(product);
        }

        let forecast = panels.section(ui, "forecast", "forecast", true, |ui| {
            self.forecast_controls(ui);
        });
        crate::witness::response(
            ui,
            hrrr_contract::Target::Panel("forecast"),
            &forecast.header,
        );
        self.water.fold(forecast.wake);

        let active_view = panels.section(ui, "active-view", "active view", true, |ui| {
            self.active_view_panel(ui);
        });
        crate::witness::response(
            ui,
            hrrr_contract::Target::Panel("active-view"),
            &active_view.header,
        );
        self.water.fold(active_view.wake);

        let views = panels.section(ui, "view-library", "views", true, |ui| {
            self.view_library_panel(ui);
        });
        crate::witness::response(ui, hrrr_contract::Target::Panel("views"), &views.header);
        self.water.fold(views.wake);

        let mut toggle_close = None;
        let application = panels.section(ui, "application", "application", true, |ui| {
            let response = ui.add_sized(
                [ui.available_width(), 26.0],
                egui::Button::new(
                    decrees::canon()
                        .spec(Decree::ToggleCloseToTray)
                        .widget_text(ui),
                )
                .selected(self.config.close_minimizes),
            );
            chrome::tension(ui, &response);
            if response.hovered() {
                self.water.hover("close-minimizes", response.rect);
            }
            if chrome::exact_activation(ui, &response) {
                toggle_close = Some(response.rect);
            }
            ui.add_space(6.0);
            let help = self.guide.activator(ui);
            crate::witness::response(ui, hrrr_contract::Target::Help, &help);
        });
        crate::witness::response(
            ui,
            hrrr_contract::Target::Panel("application"),
            &application.header,
        );
        self.water.fold(application.wake);
        if let Some(rect) = toggle_close {
            self.apply_decree(CommandDispatch::Invoke(Decree::ToggleCloseToTray));
            self.water.select(rect);
        }

        let status = panels.section(ui, "status", "status", true, |ui| {
            let _status = ui.label(chrome::muted(&self.status));
            ui.add_space(3.0);
            let _source = ui.label(chrome::muted(format!(
                "FORECAST · NOAA HRRR\nMAP · {}",
                self.basemap_status
            )));
        });
        crate::witness::response(ui, hrrr_contract::Target::Panel("status"), &status.header);
        self.water.fold(status.wake);
    }

    fn active_view_panel(&mut self, ui: &mut egui::Ui) {
        let mut edit = self.name_edit;
        let actions = library_ui::active_card(
            ui,
            &mut self.water,
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
        let mut entry_edit = self.entry_edit.take();
        let actions = library_ui::library(
            ui,
            &mut self.water,
            "view",
            &self.active_view,
            &self.views,
            &mut shelf_edit,
            &mut entry_edit,
        );
        self.shelf_edit = shelf_edit;
        self.entry_edit = entry_edit;
        self.apply_view_actions(actions);
    }

    fn forecast_controls(&mut self, ui: &mut egui::Ui) {
        let Some(run) = self.run else {
            let _waiting = ui.label(chrome::muted("finding latest forecast…"));
            return;
        };
        let run_label = run
            .local_label()
            .unwrap_or_else(|_| "invalid cycle time".to_owned());
        let valid_label = run
            .valid_local_label(self.slate.lead)
            .unwrap_or_else(|_| "invalid valid time".to_owned());
        let _run = ui.label(chrome::eyebrow(format!("RUN · {run_label}")));
        ui.add_space(3.0);

        let horizon = run.horizon().unwrap_or(LeadHour::ZERO);
        let published = self.run_extents.get(&run).copied();
        let gate = published.unwrap_or(LeadHour::ZERO);
        let cumulative = self
            .slate
            .overlay
            .active()
            .is_some_and(Product::has_baseline);
        let lead_floor = if cumulative {
            self.slate.base.next()
        } else {
            Some(LeadHour::ZERO)
        };
        let lead_ready = lead_floor.is_some_and(|floor| published.is_some() && floor <= gate);
        let mut step = None;
        let _row = ui.horizontal(|ui| {
            let previous = ui.add_enabled(
                lead_floor.is_some_and(|floor| lead_ready && self.slate.lead > floor),
                egui::Button::new("◀"),
            );
            chrome::tension(ui, &previous);
            if previous.clicked() {
                step = Some((self.slate.lead.saturating_previous(), previous.rect));
            }
            let _lead = ui.label(chrome::section_title(&valid_label));
            let next = ui.add_enabled(lead_ready && self.slate.lead < gate, egui::Button::new("▶"));
            chrome::tension(ui, &next);
            if next.clicked() {
                step = Some((self.slate.lead.saturating_next(gate), next.rect));
            }
        });
        if let Some((lead, rect)) = step {
            self.choose_lead(lead);
            self.water.lever(rect, 1.0);
        }
        let rail_ceiling = u16::from(horizon.get());
        let rail_detents = rail_ceiling + 1;
        let allowed_floor = lead_floor
            .filter(|_| lead_ready)
            .map_or(0, |floor| u16::from(floor.get()));
        let mut raw_lead = u16::from(self.slate.lead.get());
        let rail = ui
            .add_enabled_ui(lead_ready, |ui| {
                chrome::Rail::new(&mut raw_lead, 0..=rail_ceiling)
                    .allowed(allowed_floor..=u16::from(gate.get()))
                    .detents(rail_detents)
                    .show(ui)
            })
            .inner;
        crate::witness::anchor(ui, hrrr_contract::Target::ForecastHour, rail.rect);
        self.water.rail(&rail);
        if rail.changed()
            && lead_ready
            && let Ok(raw_lead) = u8::try_from(raw_lead)
            && let Ok(lead) = LeadHour::forge(raw_lead)
        {
            self.choose_lead(lead);
        }
        if published.is_none() {
            let _surveying = ui.label(chrome::muted("checking available hours…"));
        } else if !lead_ready {
            let _interval = ui.label(chrome::muted("waiting for first accumulation…"));
        }

        if cumulative {
            ui.add_space(4.0);
            let base_label = run
                .valid_local_label(self.slate.base)
                .unwrap_or_else(|_| "invalid base time".to_owned());
            let _base = ui.horizontal(|ui| {
                let _label = ui.label(chrome::muted("BASE HOUR"));
                let _value = ui.label(chrome::section_title(base_label));
            });
            let base_ceiling = self.slate.lead.saturating_previous();
            let mut raw_base = u16::from(self.slate.base.get());
            let base_rail = ui
                .add_enabled_ui(lead_ready, |ui| {
                    chrome::Rail::new(&mut raw_base, 0..=rail_ceiling)
                        .allowed(0..=u16::from(base_ceiling.get()))
                        .detents(rail_detents)
                        .show(ui)
                })
                .inner;
            crate::witness::anchor(ui, hrrr_contract::Target::BaseHour, base_rail.rect);
            self.water.rail(&base_rail);
            if base_rail.changed()
                && lead_ready
                && let Ok(raw_base) = u8::try_from(raw_base)
                && let Ok(base) = LeadHour::forge(raw_base)
            {
                self.choose_base(base);
            }
        }

        ui.add_space(4.0);
        let latest_extended = self
            .latest_run
            .map(|latest| RunSelection::LatestLong.bind(latest));
        let mut run_step = None;
        let latest = ui
            .add_enabled_ui(self.latest_run.is_some(), |ui| {
                ui.add_sized(
                    [ui.available_width(), 24.0],
                    egui::Button::new(decrees::canon().spec(Decree::FollowLatest).widget_text(ui))
                        .shortcut_text(
                            decrees::canon().shortcuts(Decree::FollowLatest)[0].label(ui.ctx()),
                        )
                        .selected(
                            self.slate.cycle == RunSelection::Latest
                                && self.latest_run == Some(run),
                        ),
                )
            })
            .inner;
        if chrome::exact_activation(ui, &latest) {
            run_step = Some((RunSelection::Latest, latest.rect));
        }
        let latest_long = ui
            .add_enabled_ui(latest_extended.is_some(), |ui| {
                ui.add_sized(
                    [ui.available_width(), 24.0],
                    egui::Button::new(
                        decrees::canon()
                            .spec(Decree::FollowLatestLong)
                            .widget_text(ui),
                    )
                    .shortcut_text(
                        decrees::canon().shortcuts(Decree::FollowLatestLong)[0].label(ui.ctx()),
                    )
                    .selected(
                        self.slate.cycle == RunSelection::LatestLong
                            && latest_extended == Some(run),
                    ),
                )
            })
            .inner;
        if chrome::exact_activation(ui, &latest_long) {
            run_step = Some((RunSelection::LatestLong, latest_long.rect));
        }
        let _step = ui.horizontal(|ui| {
            let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
            let older = ui.add_sized([width, 22.0], egui::Button::new("−1H"));
            if older.clicked() {
                run_step = Some((RunSelection::Fixed(run.hours_ago(1)), older.rect));
            }
            let newer = ui.add_enabled_ui(
                self.latest_run.is_some_and(|latest_run| run < latest_run),
                |ui| ui.add_sized([width, 22.0], egui::Button::new("+1H")),
            );
            if newer.inner.clicked() {
                let candidate = run.hours_after(1);
                run_step = self
                    .latest_run
                    .map(|latest| (RunSelection::Fixed(candidate.min(latest)), newer.inner.rect));
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
        crate::witness::anchor(ui, hrrr_contract::Target::Map, response.rect);
        self.water.begin(Domain::shelf(rect));
        let pins = self.tug_pins(ui, rect);
        let navigating = self.navigate(ui, &response, rect, pins.captured);
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
        let coherent = cover.finest_ready(|key| {
            self.tiles.contains(key)
                || self
                    .tile_rejections
                    .get(&key)
                    .is_some_and(|rejection| rejection.resolves())
        });
        let coherent = coherent.map(|stratum| stratum.keys.clone());
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
        let paints_smoke = painted_field
            .as_ref()
            .is_some_and(|(key, _field)| key.product == Product::Smoke);
        if !paints_smoke {
            self.smoke_scale.arrest();
        }
        if let Some((key, field)) = painted_field {
            if key.product == Product::Smoke {
                if navigating {
                    self.smoke_scale.arrest();
                } else {
                    let peak = self
                        .smoke_survey
                        .discern(key, &field, self.viewport, rect)
                        .map(|raw| self.scale_for(key).unit.convert(raw));
                    let proposed = self.smoke_scale.settled().reckon(peak);
                    if let Some(repaint_after) = self.smoke_scale.observe(proposed, Instant::now())
                    {
                        ui.ctx().request_repaint_after(repaint_after);
                    }
                }
            }
            let scale = self.scale_for(key).clone();
            legend_scale = Some(scale.clone());
            let _field = painter.add(egui_wgpu::Callback::new_paint_callback(
                rect,
                FieldPaint {
                    key,
                    field: field.clone(),
                    scale,
                    world_bounds: bounds,
                },
            ));
            if key.product == Product::Wind {
                wind_barb::paint(&painter, &field, self.viewport, rect);
            }
        }
        self.paint_labels(&painter, rect);
        if let Some(scale) = legend_scale.as_ref() {
            Self::legend(&painter, rect, scale);
        }
        self.scale_bar.paint(&painter, self.viewport, rect);
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
    ) -> bool {
        let before = self.viewport;
        let minimum_zoom = map::minimum_zoom(rect.height());
        self.viewport.zoom = self.viewport.zoom.max(minimum_zoom);
        let dragging = !pin_captured && response.dragged_by(egui::PointerButton::Primary);
        if dragging {
            let delta = ui.input(|input| input.pointer.delta());
            let scale = map::world_pixels(self.viewport);
            self.viewport.center_mercator[0] -= f64::from(delta.x) / scale;
            self.viewport.center_mercator[1] -= f64::from(delta.y) / scale;
            self.water.drag(rect, delta.y);
        }
        let scroll = ui.input(|input| {
            input
                .pointer
                .hover_pos()
                .filter(|pointer| rect.contains(*pointer))
                .map(|pointer| (input.smooth_scroll_delta.y, pointer))
        });
        let scrolling = scroll.is_some_and(|(scroll, _pointer)| scroll.abs() > f32::EPSILON);
        if let Some((scroll, pointer)) =
            scroll.filter(|(scroll, _pointer)| scroll.abs() > f32::EPSILON)
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
        dragging || scrolling
    }

    fn tug_pins(&mut self, ui: &egui::Ui, map_rect: egui::Rect) -> PinGesture {
        let mut gesture = PinGesture::default();
        let scale = map::world_pixels(self.viewport);
        let mut moved = false;
        let mut seized_any = false;
        for slot in 0..self.pins.len() {
            let pin = self.pins[slot];
            let anchor = map::screen_at(self.viewport, map_rect, pin.world());
            let hardware = chrome::ForgePin::new(anchor).size(chrome::MechanismSize::Medium);
            let response = ui.interact(
                hardware.grip(),
                egui::Id::new(("pin-bulb", slot)),
                egui::Sense::drag(),
            );
            crate::witness::anchor(ui, hrrr_contract::Target::Pin(slot), response.rect);
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
        if !seized_any
            && let Some(tug) = self.pin_tug.take()
            && let Some(before) = tug.reversal(&self.pins)
        {
            self.map_undo.remember(MapReversal::Pins {
                view: self.active_view.clone(),
                before,
            });
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
            self.remember_pins();
            self.pins.push(point);
            self.sync_active_view();
        } else if self.transient_probe != Some(point) {
            self.remember_probe();
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
            self.remember_pins();
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
        let mut phalanx = PlaquePhalanx::forge(map_rect);
        let mut victims = Vec::new();
        for slot in 0..self.pins.len() {
            let pin = self.pins[slot];
            let anchor = map::screen_at(self.viewport, map_rect, pin.world());
            if !map_rect.expand(8.0).contains(anchor) {
                continue;
            }
            let hardware = chrome::ForgePin::new(anchor).size(chrome::MechanismSize::Medium);
            let crown = hardware.bulb();
            hardware.paint(painter, hot_pin == Some(slot));
            let id = egui::Id::new(("persistent-pin", slot));
            let size = ctx
                .memory(|memory| memory.area_rect(id).map(|rect| rect.size()))
                .unwrap_or_default();
            let berth = phalanx.berth(crown, size, egui::vec2(11.0, 7.0));
            let response = self.point_popup(ctx, id, berth, pin, key, field.as_ref(), true);
            phalanx.occupy(response.rect);
            if response.reap {
                victims.push(slot);
            }
        }

        if let Some(probe) = self.transient_probe {
            let anchor = map::screen_at(self.viewport, map_rect, probe.world());
            if map_rect.expand(8.0).contains(anchor) {
                let _dot = painter.circle_filled(anchor, 3.25, Color32::from_rgb(45, 42, 37));
                let _rim = painter.circle_stroke(
                    anchor,
                    3.25,
                    egui::Stroke::new(1.0_f32, chrome::SURFACE),
                );
                crate::witness::rect(
                    ctx,
                    hrrr_contract::Target::TransientProbe,
                    egui::Rect::from_center_size(anchor, egui::Vec2::splat(8.0)),
                );
                let id = egui::Id::new("transient-probe");
                let size = ctx
                    .memory(|memory| memory.area_rect(id).map(|rect| rect.size()))
                    .unwrap_or_default();
                let berth = phalanx.berth(anchor, size, egui::vec2(9.0, 9.0));
                let popup = self.point_popup(ctx, id, berth, probe, key, field.as_ref(), false);
                phalanx.occupy(popup.rect);
            }
        }
        if !victims.is_empty() {
            self.remember_pins();
            for victim in victims.into_iter().rev() {
                let _reaped = self.pins.remove(victim);
            }
            self.sync_active_view();
        }
    }

    #[cfg(feature = "egui-test")]
    pub fn witness_state(&self) -> crate::witness::State {
        crate::witness::State {
            contract: hrrr_contract::UI_FINGERPRINT,
            launch: "ready",
            active_field: self
                .slate
                .overlay
                .active()
                .map(|product| product.cache_name().to_owned()),
            lead_hour: self.slate.lead.get(),
            base_hour: self
                .slate
                .overlay
                .active()
                .filter(|product| product.has_baseline())
                .map(|_| self.slate.base.get()),
            active_view: self.active_view.as_str().to_owned(),
            pins: self.pins.iter().map(|pin| pin.world()).collect(),
            transient_probe: self.transient_probe.map(MercatorPoint::world),
            dragging_pin: self.pin_tug.map(|tug| tug.slot),
            guide_open: self.guide.is_open(),
            close_to_tray: self.config.close_minimizes,
            viewport: crate::witness::Viewport {
                center: self.viewport.center_mercator,
                zoom: self.viewport.zoom,
            },
        }
    }

    fn point_popup(
        &mut self,
        ctx: &egui::Context,
        id: egui::Id,
        berth: PlaqueBerth,
        point: MercatorPoint,
        key: Option<FrameKey>,
        field: Option<&(FrameKey, Arc<FieldGrid>)>,
        removable: bool,
    ) -> PlaqueResponse {
        let sample = field.and_then(|(_, field)| sample_point(point, field));
        let mut reap = false;
        let popup = egui::Area::new(id)
            .order(egui::Order::Foreground)
            .pivot(berth.pivot)
            .fixed_pos(berth.position)
            .constrain(false)
            .show(ctx, |ui| {
                let close = removable
                    .then(|| chrome::CornerClose::new().size(chrome::MechanismSize::Small));
                let margin = egui::Margin::symmetric(8, 6);
                let pane = egui::Frame::new()
                    .fill(chrome::SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, chrome::EDGE_STRONG))
                    .inner_margin(margin)
                    .show(ui, |ui| {
                        let valid = key
                            .map(|key| (key.run, key.valid))
                            .or_else(|| self.run.map(|run| (run, self.slate.lead)))
                            .map_or_else(
                                || "NO FORECAST".to_owned(),
                                |(run, lead)| {
                                    run.valid_local_label(lead)
                                        .unwrap_or_else(|_| "INVALID TIME".to_owned())
                                },
                            );
                        if let Some(close) = close {
                            let _time = close
                                .guarded_header(ui, margin, |ui| ui.label(chrome::eyebrow(valid)));
                        } else {
                            let _time = ui.label(chrome::eyebrow(valid));
                        }
                        if let (Some(key), Some(raw)) = (key, sample) {
                            let scale = self.scale_for(key);
                            let _value = ui.label(chrome::section_title(scale.display(raw)));
                        } else if self.slate.overlay.active().is_some() {
                            let _pending = ui.label(chrome::muted(if field.is_some() {
                                "OUTSIDE FORECAST AREA"
                            } else {
                                "UPDATING…"
                            }));
                        }
                        let [longitude, latitude] = map::lon_lat_at(point.world());
                        let _position =
                            ui.label(chrome::muted(format!("{latitude:.4}°, {longitude:.4}°")));
                    });
                if let Some(close) = close {
                    let close = close
                        .show(ui, pane.response.rect, (id, "close"))
                        .on_hover_text("remove pin");
                    self.water.corner_close(&close);
                    reap = close.clicked();
                }
            });
        PlaqueResponse {
            reap,
            rect: popup.response.rect,
        }
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
            let maximum = scale.unit.format_ceiling(last.ceiling);
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

    fn absorb_events(&mut self, ctx: &egui::Context) {
        let mut drain = EVENT_DRAIN.arm();
        while let Some(message) = drain.receive(&self.custodian.faults) {
            self.status = message;
        }
        while let Some(event) = drain.receive(&self.worker.events) {
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
                        "latest forecast found".clone_into(&mut self.status);
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
                        self.clamp_clock();
                        let active = self.active_key();
                        if self.slate.overlay.active().is_some()
                            && (active.is_none()
                                || self
                                    .displayed_field
                                    .as_ref()
                                    .is_none_or(|(key, _field)| Some(*key) != active))
                        {
                            self.demand_active();
                        } else if prior != Some(extent.published()) {
                            "new forecast hours available".clone_into(&mut self.status);
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
                Event::Loaded { demand, field } => {
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
                        "forecast ready".clone_into(&mut self.status);
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
        while let Some(event) = drain.receive(&self.basemap.events) {
            match event {
                basemap::Event::Ready => {
                    "PROTOMAPS · © OPENSTREETMAP CONTRIBUTORS".clone_into(&mut self.basemap_status);
                }
                basemap::Event::Loaded(tile) => {
                    let key = tile.key;
                    let _was_inflight = self.tile_inflight.remove(&key);
                    let _rejection = self.tile_rejections.remove(&key);
                    self.tiles.insert(tile);
                }
                basemap::Event::Missing(key) => {
                    let _was_inflight = self.tile_inflight.remove(&key);
                    let _prior = self.tile_rejections.insert(key, TileRejection::Absent);
                }
                basemap::Event::Fault { key, message } => {
                    let detail = key.is_some_and(TileKey::is_detail);
                    if let Some(key) = key {
                        let _was_inflight = self.tile_inflight.remove(&key);
                        let _prior = self.tile_rejections.insert(
                            key,
                            TileRejection::RetryAt(Instant::now() + TILE_RETRY_DELAY),
                        );
                    }
                    self.basemap_status = if detail {
                        format!("MAP DETAIL OFFLINE · {message}")
                    } else {
                        format!("MAP UNAVAILABLE · {message}")
                    };
                }
            }
        }
        if !self.custodian.faults.is_empty()
            || !self.worker.events.is_empty()
            || !self.basemap.events.is_empty()
        {
            ctx.request_repaint();
        }
    }

    fn survey_deadline(&self) -> Option<Instant> {
        let run = self.run?;
        let incomplete = self
            .run_extents
            .get(&run)
            .is_some_and(|published| run.horizon().is_ok_and(|horizon| *published < horizon));
        (incomplete && self.surveying_run.is_none() && self.loading.is_none())
            .then_some(self.next_survey)
    }

    fn request_survey(&mut self, run: RunId) {
        if self.surveying_run == Some(run) {
            return;
        }
        self.next_survey = Instant::now() + FRONTIER_POLL;
        match self.worker.send(Command::Survey(run)) {
            Ok(()) => {
                self.surveying_run = Some(run);
                "checking available forecast hours…".clone_into(&mut self.status);
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn request_discovery(&mut self) {
        match self.worker.send(Command::Discover) {
            Ok(()) => {
                self.announced_discovery = true;
                "finding latest forecast…".clone_into(&mut self.status);
            }
            Err(err) => self.status = err.to_string(),
        }
    }

    fn decree_status(&self, decree: Decree) -> CommandStatus<'static> {
        match decree {
            Decree::FollowLatest if self.latest_run.is_none() => {
                CommandStatus::Disabled("available forecasts are still loading")
            }
            Decree::FollowLatestLong
                if self
                    .latest_run
                    .map(|latest| RunSelection::LatestLong.bind(latest))
                    .is_none() =>
            {
                CommandStatus::Disabled("no 48-hour run is available")
            }
            Decree::UndoMapChange if !self.map_undo.has_reversal_for(&self.active_view) => {
                CommandStatus::Disabled("the active view has no map change to undo")
            }
            Decree::FollowLatest
            | Decree::FollowLatestLong
            | Decree::UndoMapChange
            | Decree::ToggleCloseToTray => CommandStatus::Enabled,
        }
    }

    fn apply_decree(&mut self, dispatch: CommandDispatch<'_, Decree>) {
        let decree = match dispatch {
            CommandDispatch::Invoke(decree) => decree,
            CommandDispatch::Refused { reason, .. } => {
                self.status = format!("unavailable: {reason}");
                return;
            }
        };
        match decree {
            Decree::FollowLatest => self.follow_cycle(RunSelection::Latest),
            Decree::FollowLatestLong => self.follow_cycle(RunSelection::LatestLong),
            Decree::UndoMapChange => self.undo_map_object(),
            Decree::ToggleCloseToTray => {
                self.config.close_minimizes = !self.config.close_minimizes;
                let status = if self.config.close_minimizes {
                    "close hides the window"
                } else {
                    "close quits"
                };
                status.clone_into(&mut self.status);
                self.mark_config_dirty();
            }
        }
    }

    fn take_keys(&mut self, ctx: &egui::Context) {
        if self.guide.is_open() || ctx.memory(|memory| memory.top_modal_layer().is_some()) {
            return;
        }
        let text_edit_focused = ctx.text_edit_focused();
        if !text_edit_focused {
            while let Some((slot, assign)) = consume_view_slot(ctx) {
                if assign {
                    self.assign_view_slot(slot);
                } else {
                    self.load_view_slot(slot);
                }
            }
        }
        if !text_edit_focused
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            && self.transient_probe.is_some()
        {
            self.remember_probe();
            self.transient_probe = None;
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
                ViewAction::RenameEntry { from, to } => {
                    let _renamed = self.rename_view_to(&from, to);
                }
                ViewAction::Moor { name, berth } => {
                    self.views.moor(&name, &berth);
                    self.mark_views_dirty();
                }
                ViewAction::MoorShelf { shelf, berth } => {
                    self.views.moor_shelf(shelf, berth);
                    self.shelf_edit = None;
                    self.mark_views_dirty();
                }
                ViewAction::NewShelf => {
                    self.views.add_shelf();
                    self.mark_views_dirty();
                }
                ViewAction::ToggleShelf(shelf) => {
                    self.views.toggle_shelf(shelf);
                    self.slate.closed_folders = self.views.closed_shelves();
                    self.mark_dirty();
                }
                ViewAction::ScuttleShelf(shelf) => {
                    self.views.scuttle_shelf(shelf);
                    self.shelf_edit = None;
                    self.slate.closed_folders = self.views.closed_shelves();
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
                            self.slate.closed_folders = self.views.closed_shelves();
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

    fn remember_pins(&mut self) {
        self.map_undo.remember(MapReversal::Pins {
            view: self.active_view.clone(),
            before: self.pins.clone(),
        });
    }

    fn remember_probe(&mut self) {
        self.map_undo.remember(MapReversal::Probe {
            before: self.transient_probe,
        });
    }

    fn undo_map_object(&mut self) {
        let Some(recoil) =
            self.map_undo
                .recoil(&self.active_view, &self.pins, self.transient_probe)
        else {
            return;
        };
        self.pin_tug = None;
        match recoil {
            MapRecoil::Pins(pins) => {
                self.pins = pins;
                self.sync_active_view();
            }
            MapRecoil::Probe(probe) => self.transient_probe = probe,
        }
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
        self.pin_tug = None;
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
        self.map_undo.forget_view(&removed.name);
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
        if self.rename_view_to(&old, new) {
            self.view_name_entry.clear();
            self.name_edit = NameEdit::Idle;
        }
    }

    fn rename_view_to(&mut self, old: &EntryName, new: EntryName) -> bool {
        if old == &new {
            return true;
        }
        if self.views.taken(&new) {
            self.status = format!("view `{new}` already exists");
            return false;
        }
        if old == &self.active_view {
            self.sync_active_view();
        }
        if !self.views.rename(old, new.clone()) {
            return false;
        }
        self.map_undo.rename_view(old, &new);
        if old == &self.active_view {
            self.active_view = new.clone();
            self.slate.active_view = Some(new.clone());
        }
        self.status = format!("renamed view `{old}` → `{new}`");
        self.mark_dirty();
        self.mark_views_dirty();
        true
    }

    fn strike_overlay(&mut self, product: Product) {
        self.slate.overlay = self.slate.overlay.strike(product);
        self.mark_dirty();
        if self.slate.overlay.active().is_some() {
            self.clamp_clock();
            self.demand_active();
        } else {
            self.demand_id.advance();
            self.loading = None;
            self.displayed_field = None;
            self.prefetch.clear();
            "map only".clone_into(&mut self.status);
        }
    }

    fn choose_lead(&mut self, lead: LeadHour) {
        let Some(frontier) = self.run.and_then(|run| self.run_extents.get(&run)).copied() else {
            return;
        };
        let floor = self
            .slate
            .overlay
            .active()
            .filter(|product| product.has_baseline())
            .map_or(Some(LeadHour::ZERO), |_| self.slate.base.next());
        let Some(floor) = floor.filter(|floor| *floor <= frontier) else {
            return;
        };
        let lead = lead.clamp(floor, frontier);
        if self.slate.lead != lead {
            self.slate.lead = lead;
            self.mark_dirty();
            self.demand_active();
        }
    }

    fn choose_base(&mut self, base: LeadHour) {
        let cumulative = self
            .slate
            .overlay
            .active()
            .is_some_and(Product::has_baseline);
        if !cumulative || self.slate.lead == LeadHour::ZERO {
            return;
        }
        let base = base.min(self.slate.lead.saturating_previous());
        if self.slate.base != base {
            self.slate.base = base;
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
                self.clamp_clock();
                self.demand_active();
            } else {
                self.demand_id.advance();
                self.prefetch.clear();
                self.request_survey(run);
            }
        }
    }

    fn reconcile_forecast(&mut self) {
        self.clamp_clock();
        let active = self.active_key();
        if self.slate.overlay.active().is_some()
            && (active.is_none()
                || self
                    .displayed_field
                    .as_ref()
                    .is_none_or(|(key, _field)| Some(*key) != active))
        {
            self.demand_active();
        }
    }

    fn clamp_clock(&mut self) {
        let Some(run) = self.run else {
            return;
        };
        let Some(ceiling) = self
            .run_extents
            .get(&run)
            .copied()
            .or_else(|| run.horizon().ok())
        else {
            return;
        };
        let (lead, base) = lawful_clock(
            self.slate.overlay.active(),
            self.slate.lead,
            self.slate.base,
            ceiling,
        );
        if (lead, base) != (self.slate.lead, self.slate.base) {
            self.slate.lead = lead;
            self.slate.base = base;
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
        let (lead, base) = self
            .run
            .map_or((self.slate.lead, self.slate.base), |source| {
                (
                    run.rebase_lead(source, self.slate.lead, frontier),
                    run.rebase_lead(source, self.slate.base, frontier),
                )
            });
        let (lead, base) = lawful_clock(self.slate.overlay.active(), lead, base, frontier);
        self.run = Some(run);
        self.slate.lead = lead;
        self.slate.base = base;
    }

    fn active_key(&self) -> Option<FrameKey> {
        let run = self.run?;
        let published = self.run_extents.get(&run)?;
        let product = self.slate.overlay.active()?;
        (self.slate.lead <= *published)
            .then(|| FrameKey::forge(run, product, self.slate.base, self.slate.lead))
            .flatten()
    }

    fn scale_for(&self, key: FrameKey) -> &Scale {
        self.scales.get(
            key.product,
            self.smoke_scale.settled(),
            TemperatureSeason::at(key),
        )
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
        let rejected = match self.tile_rejections.get(&key).copied() {
            Some(rejection) if rejection.blocks(Instant::now()) => true,
            Some(_) => {
                let _expired = self.tile_rejections.remove(&key);
                false
            }
            None => false,
        };
        if !self.tiles.contains(key)
            && !self.tile_inflight.contains(&key)
            && !rejected
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
            self.demand_id.advance();
            self.loading = None;
            self.prefetch.clear();
            self.displayed_field = None;
            return;
        };
        self.demand_id.advance();
        self.prefetch.clear();
        self.seed_prefetch(key);
        if self.fields.get(key).is_some() {
            self.loading = None;
            self.displayed_field = self.fields.get(key).map(|field| (key, field.clone()));
            "forecast ready".clone_into(&mut self.status);
            self.kick_prefetch();
        } else {
            let demand = LoadDemand {
                intent: LoadIntent::Foreground(self.demand_id),
                key,
            };
            self.loading = Some(demand);
            "loading forecast…".clone_into(&mut self.status);
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
            if let Ok(lead) = LeadHour::forge(key.valid.get().saturating_add(distance))
                && lead <= horizon
                && let Some(frame) = key.with_valid(lead)
            {
                self.prefetch.push_back(frame);
            }
            if key.valid.get() >= distance
                && let Ok(lead) = LeadHour::forge(key.valid.get() - distance)
                && let Some(frame) = key.with_valid(lead)
            {
                self.prefetch.push_back(frame);
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
        self.dirty.slate = true;
        self.scribe.mark();
    }

    fn mark_config_dirty(&mut self) {
        self.dirty.config = true;
        self.scribe.mark();
    }

    fn mark_views_dirty(&mut self) {
        self.dirty.views = true;
        self.scribe.mark();
    }

    fn durable_state(&self) -> DurableState {
        DurableState {
            slate: self.dirty.slate.then(|| self.slate.clone()),
            config: self.dirty.config.then(|| self.config.clone()),
            views: self.dirty.views.then(|| self.views.clone()),
        }
    }

    fn durable_state_all(&self) -> DurableState {
        DurableState {
            slate: Some(self.slate.clone()),
            config: Some(self.config.clone()),
            views: Some(self.views.clone()),
        }
    }

    fn absorb_persistence(&mut self) {
        if let Some(ScribeOutcome::Fault { message, .. }) = self.scribe.take_outcome() {
            self.dirty = DirtyState {
                slate: true,
                config: true,
                views: true,
            };
            self.status = format!("state save failed: {message}");
        }
    }
}

impl Drop for WeatherApp {
    fn drop(&mut self) {
        self.retire();
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

fn lawful_clock(
    product: Option<Product>,
    mut lead: LeadHour,
    mut base: LeadHour,
    ceiling: LeadHour,
) -> (LeadHour, LeadHour) {
    lead = lead.min(ceiling);
    if product.is_some_and(Product::has_baseline) {
        if ceiling == LeadHour::ZERO {
            return (LeadHour::ZERO, LeadHour::ZERO);
        }
        if lead == LeadHour::ZERO {
            lead = LeadHour::ONE;
        }
        base = base.min(lead.saturating_previous());
    }
    (lead, base)
}

#[cfg(feature = "egui-test")]
fn witnessed_frontier(run: Option<RunId>) -> Option<(RunId, LeadHour)> {
    let run = run?;
    let raw = std::env::var("HRRR_ACCEPTANCE_PUBLISHED").ok()?;
    let published = raw
        .parse::<u8>()
        .ok()
        .and_then(|hour| LeadHour::forge(hour).ok())?;
    Some((run, published))
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
    use anyhow::Context as _;

    fn point(x: f64) -> Result<MercatorPoint> {
        MercatorPoint::forge([x, 0.5]).context("test map point")
    }

    fn view(name: &str) -> Result<EntryName> {
        EntryName::forge(name).context("test view name")
    }

    #[test]
    fn scale_latch_requires_quiet_after_navigation() {
        // A pan can cross both smoke thresholds in one frame. Value hysteresis
        // alone therefore cannot keep the field and legend stable under drag.
        let begun = Instant::now();
        let mut latch = ScaleLatch::<SmokeRegime>::default();

        assert_eq!(latch.observe(SmokeRegime::Heavy, begun), Some(SCALE_SETTLE));
        latch.arrest();
        let released = begun + SCALE_SETTLE;
        assert_eq!(
            latch.observe(SmokeRegime::Heavy, released),
            Some(SCALE_SETTLE)
        );
        assert_eq!(
            latch.observe(
                SmokeRegime::Heavy,
                released + SCALE_SETTLE.saturating_sub(Duration::from_millis(1)),
            ),
            Some(Duration::from_millis(1))
        );
        assert_eq!(latch.settled(), SmokeRegime::Light);
        assert_eq!(
            latch.observe(SmokeRegime::Heavy, released + SCALE_SETTLE),
            None
        );
        assert_eq!(latch.settled(), SmokeRegime::Heavy);
    }

    #[test]
    fn map_undo_interleaves_global_probe_and_view_owned_pins() -> Result<()> {
        let alpha = view("alpha")?;
        let beta = view("beta")?;
        let alpha_pin = point(0.2)?;
        let beta_pin = point(0.7)?;
        let probe = point(0.4)?;
        let mut undo = MapUndo::default();
        undo.remember(MapReversal::Pins {
            view: alpha.clone(),
            before: Vec::new(),
        });
        undo.remember(MapReversal::Pins {
            view: beta.clone(),
            before: Vec::new(),
        });
        undo.remember(MapReversal::Probe { before: None });

        assert_eq!(
            undo.recoil(&alpha, &[alpha_pin], Some(probe)),
            Some(MapRecoil::Probe(None))
        );
        assert_eq!(
            undo.recoil(&alpha, &[alpha_pin], None),
            Some(MapRecoil::Pins(Vec::new()))
        );
        assert_eq!(undo.recoil(&alpha, &[], None), None);
        assert_eq!(
            undo.recoil(&beta, &[beta_pin], None),
            Some(MapRecoil::Pins(Vec::new()))
        );
        Ok(())
    }

    #[test]
    fn pin_tug_forges_one_reversal_only_for_net_motion() -> Result<()> {
        let origin = point(0.2)?;
        let moved = point(0.3)?;
        let tug = PinTug {
            slot: 0,
            origin,
            world_points: 1.0,
        };
        assert_eq!(tug.reversal(&[origin]), None);
        assert_eq!(tug.reversal(&[moved]), Some(vec![origin]));
        Ok(())
    }

    #[test]
    fn map_undo_tracks_view_renames_and_forgets_deleted_views() -> Result<()> {
        let old = view("old")?;
        let new = view("new")?;
        let pin = point(0.2)?;
        let mut undo = MapUndo::default();
        undo.remember(MapReversal::Pins {
            view: old.clone(),
            before: Vec::new(),
        });
        undo.rename_view(&old, &new);
        assert_eq!(
            undo.recoil(&new, &[pin], None),
            Some(MapRecoil::Pins(Vec::new()))
        );

        undo.remember(MapReversal::Pins {
            view: new.clone(),
            before: Vec::new(),
        });
        undo.remember(MapReversal::Probe { before: None });
        undo.forget_view(&new);
        assert_eq!(
            undo.recoil(&new, &[], Some(pin)),
            Some(MapRecoil::Probe(None))
        );
        assert_eq!(undo.recoil(&new, &[pin], None), None);
        Ok(())
    }

    #[test]
    fn plaque_phalanx_uses_the_clear_flank_then_keeps_anchor_adjacency() {
        let mut phalanx = PlaquePhalanx::forge(egui::Rect::from_min_max(
            egui::pos2(0.0, 0.0),
            egui::pos2(500.0, 300.0),
        ));
        let size = egui::vec2(120.0, 60.0);
        let gap = egui::vec2(10.0, 5.0);
        let first = phalanx.berth(egui::pos2(200.0, 100.0), size, gap);
        assert_eq!(first.pivot, egui::Align2::LEFT_TOP);
        phalanx.occupy(first.rect(size));

        let second = phalanx.berth(egui::pos2(210.0, 100.0), size, gap);
        assert_eq!(second.pivot, egui::Align2::RIGHT_TOP);
        assert_eq!(second.rect(size).right(), 200.0);
        assert!(!first.rect(size).intersects(second.rect(size)));

        let mut blocked = PlaquePhalanx::forge(egui::Rect::from_min_max(
            egui::pos2(0.0, 0.0),
            egui::pos2(500.0, 300.0),
        ));
        let anchor = egui::pos2(250.0, 100.0);
        blocked.occupy(egui::Rect::from_min_max(
            egui::pos2(0.0, 90.0),
            egui::pos2(500.0, 180.0),
        ));

        let berth = blocked.berth(anchor, size, gap);
        assert_eq!(berth.pivot, egui::Align2::LEFT_TOP);
        assert_eq!(berth.rect(size).left(), anchor.x + gap.x);
    }

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

    #[test]
    fn absent_tiles_are_final_while_faults_cool_down() {
        let now = Instant::now();
        assert!(TileRejection::Absent.blocks(now));
        assert!(TileRejection::Absent.resolves());
        assert!(TileRejection::RetryAt(now + TILE_RETRY_DELAY).blocks(now));
        assert!(!TileRejection::RetryAt(now).resolves());
        assert!(!TileRejection::RetryAt(now).blocks(now));
    }

    #[test]
    fn cumulative_clock_preserves_valid_time_and_lowers_an_illegal_base() -> Result<()> {
        let hour = |value| LeadHour::forge(value);
        assert_eq!(
            lawful_clock(Some(Product::QpfRun), hour(5)?, hour(8)?, hour(18)?),
            (hour(5)?, hour(4)?)
        );
        assert_eq!(
            lawful_clock(Some(Product::QpfRun), hour(0)?, hour(8)?, hour(18)?),
            (hour(1)?, hour(0)?)
        );
        assert_eq!(
            lawful_clock(Some(Product::QpfRun), hour(8)?, hour(3)?, hour(0)?),
            (hour(0)?, hour(0)?)
        );
        assert_eq!(
            lawful_clock(Some(Product::Smoke), hour(8)?, hour(6)?, hour(4)?),
            (hour(4)?, hour(6)?)
        );
        Ok(())
    }
}
