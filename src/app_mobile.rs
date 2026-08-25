//! Android's bottom Inspector and touch projection.

use super::*;
use crate::state::MobilePanel;

const PANELS: [MobilePanel; 6] = [
    MobilePanel::Application,
    MobilePanel::Field,
    MobilePanel::Forecast,
    MobilePanel::ActiveView,
    MobilePanel::Views,
    MobilePanel::Status,
];
const PANEL_WIDTH: f32 = 340.0;
const PANEL_BUTTON_TRAVEL: f32 = 32.0;
const COLLAPSED_HEIGHT: f32 = 42.0;
const MIN_EXPANDED_HEIGHT: f32 = 220.0;
const SWIPE_THRESHOLD: f32 = 64.0;

const MOBILE_INSPECTOR_GESTURES: [TouchGesture; 3] = [
    TouchGesture::new(
        "SWIPE ↔",
        "Switch panel",
        "Moves to the adjacent Inspector panel.",
    ),
    TouchGesture::new(
        "SWIPE ↕",
        "Scroll panel",
        "Moves vertically through the current panel.",
    ),
    TouchGesture::new(
        "TAP CENTER ARROW",
        "Lower or raise Inspector",
        "Leaves the map exposed or restores the current panel.",
    ),
];
const MOBILE_MAP_GESTURES: [TouchGesture; 5] = [
    TouchGesture::new("DRAG", "Pan map", "Moves the map beneath one finger."),
    TouchGesture::new(
        "PINCH",
        "Zoom map",
        "Changes scale around the gesture's center.",
    ),
    TouchGesture::new(
        "TAP",
        "Probe forecast",
        "Places or moves a temporary reading.",
    ),
    TouchGesture::new("TAP PROBE", "Clear probe", "Removes the temporary reading."),
    TouchGesture::new(
        "HOLD / DRAG PIN",
        "Keep or move pin",
        "Touch and hold the map to keep a reading; drag its forged head to move it.",
    ),
];
const MOBILE_VIEW_GESTURES: [TouchGesture; 1] = [TouchGesture::new(
    "DRAG HANDLE",
    "Rearrange views",
    "Moves a view or folder to the indicated berth.",
)];
const MOBILE_GUIDE_GROUPS: [TouchGuideGroup; 3] = [
    TouchGuideGroup::new("INSPECTOR", &MOBILE_INSPECTOR_GESTURES),
    TouchGuideGroup::new("MAP", &MOBILE_MAP_GESTURES),
    TouchGuideGroup::new("VIEWS", &MOBILE_VIEW_GESTURES),
];

#[derive(Debug)]
pub(super) struct MobileInspector {
    first: usize,
    expanded: bool,
    swipe: Option<Swipe>,
    platform_offset: f32,
}

#[derive(Debug)]
struct Swipe {
    device: egui::TouchDeviceId,
    id: egui::TouchId,
    origin: egui::Pos2,
    last: egui::Pos2,
}

struct MobileInspectorResponse {
    controls: Vec<chrome::MonoglyphResponse>,
    domain: egui::Rect,
    panel: MobilePanel,
    platform_offset: f32,
    undo_clicked: bool,
}

impl Default for MobileInspector {
    fn default() -> Self {
        Self {
            first: 1,
            expanded: true,
            swipe: None,
            platform_offset: 0.0,
        }
    }
}

impl MobileInspector {
    pub(super) fn restore(session: &SessionState) -> Self {
        let panel = if session.overlay.active().is_some() {
            MobilePanel::Forecast
        } else {
            session.mobile_panel.unwrap_or(MobilePanel::Field)
        };
        Self {
            first: PANELS
                .iter()
                .position(|candidate| *candidate == panel)
                .expect("mobile panel vocabulary contains every durable panel"),
            ..Self::default()
        }
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        undo_enabled: bool,
        mut add: impl FnMut(&mut egui::Ui, usize),
    ) -> MobileInspectorResponse {
        let available = ui.available_rect_before_wrap();
        let columns = ((available.width() / PANEL_WIDTH).floor() as usize).clamp(1, PANELS.len());
        let last_start = PANELS.len().saturating_sub(columns);
        self.first = self.first.min(last_start);

        let maximum = (available.height() * 0.68).max(MIN_EXPANDED_HEIGHT);
        let desired = (available.height() * 0.44).clamp(MIN_EXPANDED_HEIGHT, maximum);
        let collapsed = egui::Panel::bottom("forecast-inspector-mobile-collapsed")
            .resizable(false)
            .exact_size(COLLAPSED_HEIGHT)
            .show_separator_line(true);
        let expanded = egui::Panel::bottom("forecast-inspector-mobile-expanded")
            .resizable(false)
            .exact_size(desired)
            .show_separator_line(true);

        let mut expanded_state = self.expanded;
        let mut requested_expansion = None;
        let mut undo_clicked = false;
        let first = &mut self.first;
        let swipe = &mut self.swipe;
        let platform_offset = &mut self.platform_offset;
        let mut controls = Vec::with_capacity(3);
        let panel = egui::Panel::show_switched(
            ui,
            &mut expanded_state,
            collapsed,
            expanded,
            |ui, showing_expanded| {
                if showing_expanded {
                    show_expanded(
                        ui,
                        first,
                        swipe,
                        platform_offset,
                        columns,
                        last_start,
                        &mut requested_expansion,
                        &mut controls,
                        &mut add,
                    );
                } else {
                    let (_title, raise, undo) = navigation_row(
                        ui,
                        "collapsed",
                        |ui| ui.label(chrome::section_title(PANELS[*first].name().to_uppercase())),
                        |ui| {
                            chrome::Monoglyph::symbol(chrome::Symbol::ArrowUp)
                                .show(ui)
                                .on_hover_text("Raise the Inspector")
                        },
                        |ui| {
                            ui.add_enabled_ui(undo_enabled, |ui| {
                                chrome::Monoglyph::symbol(chrome::Symbol::Undo).show(ui)
                            })
                            .inner
                            .on_hover_text("Undo the last pin or probe change")
                        },
                    );
                    if raise.clicked() {
                        requested_expansion = Some(true);
                    }
                    undo_clicked = undo.clicked();
                    controls.extend([raise, undo]);
                }
            },
        );
        self.expanded = requested_expansion.unwrap_or(expanded_state);
        if self.expanded != expanded_state {
            ui.ctx().request_repaint();
        }
        MobileInspectorResponse {
            controls,
            domain: panel.response.rect,
            panel: PANELS[self.first],
            platform_offset: self.platform_offset,
            undo_clicked,
        }
    }
}

fn show_expanded(
    ui: &mut egui::Ui,
    first: &mut usize,
    swipe: &mut Option<Swipe>,
    platform_offset: &mut f32,
    columns: usize,
    last_start: usize,
    requested_expansion: &mut Option<bool>,
    controls: &mut Vec<chrome::MonoglyphResponse>,
    add: &mut impl FnMut(&mut egui::Ui, usize),
) {
    let shown_end = (*first + columns).min(PANELS.len());
    let position = if columns == 1 {
        format!(
            "{} · {}/{}",
            PANELS[*first].name().to_uppercase(),
            *first + 1,
            PANELS.len()
        )
    } else {
        format!(
            "{} · {}–{}/{}",
            PANELS[*first].name().to_uppercase(),
            *first + 1,
            shown_end,
            PANELS.len()
        )
    };
    let previous_enabled = *first > 0;
    let next_enabled = *first < last_start;
    let (previous, lower, next) = navigation_row(
        ui,
        "expanded",
        |ui| {
            let previous = ui
                .add_enabled_ui(previous_enabled, |ui| {
                    chrome::Monoglyph::symbol(chrome::Symbol::ArrowLeft).show(ui)
                })
                .inner
                .on_hover_text("Previous Inspector panel");
            let _position = ui.label(chrome::section_title(&position));
            previous
        },
        |ui| {
            chrome::Monoglyph::symbol(chrome::Symbol::ArrowDown)
                .show(ui)
                .on_hover_text("Lower the Inspector")
        },
        |ui| {
            ui.add_enabled_ui(next_enabled, |ui| {
                chrome::Monoglyph::symbol(chrome::Symbol::ArrowRight).show(ui)
            })
            .inner
            .on_hover_text("Next Inspector panel")
        },
    );
    if previous.clicked() {
        if step_panel(first, last_start, -1) {
            *platform_offset += PANEL_BUTTON_TRAVEL;
        }
    }
    if next.clicked() {
        if step_panel(first, last_start, 1) {
            *platform_offset -= PANEL_BUTTON_TRAVEL;
        }
    }
    if lower.clicked() {
        *requested_expansion = Some(false);
    }
    controls.extend([previous, lower, next]);

    let body = ui.available_rect_before_wrap();
    let mut scroll_drag_ids = Vec::with_capacity(columns);
    ui.columns(columns, |columns_ui| {
        for (column, panel_ui) in columns_ui.iter_mut().enumerate() {
            let panel = *first + column;
            let _title = panel_ui
                .label(chrome::section_title(PANELS[panel].name().to_uppercase()).size(16.0));
            let _separator = panel_ui.separator();
            let scroll = egui::ScrollArea::vertical()
                .id_salt(("mobile-inspector-panel", panel))
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .scroll_source(egui::scroll_area::ScrollSource::ALL)
                .auto_shrink([false, false])
                .show(panel_ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui, panel);
                });
            scroll_drag_ids.push(scroll.id.with("area"));
        }
    });

    let touches = ui.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Touch {
                    device_id,
                    id,
                    phase,
                    pos,
                    ..
                } => Some((*device_id, *id, *phase, *pos)),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    let claimed_by_control = egui::DragAndDrop::has_any_payload(ui.ctx())
        || ui
            .ctx()
            .dragged_id()
            .or_else(|| ui.ctx().drag_stopped_id())
            .is_some_and(|id| !scroll_drag_ids.contains(&id));
    if claimed_by_control {
        *swipe = None;
    }
    for (device, id, phase, pos) in touches.into_iter().filter(|_| !claimed_by_control) {
        match phase {
            egui::TouchPhase::Start if body.contains(pos) && swipe.is_none() => {
                *swipe = Some(Swipe {
                    device,
                    id,
                    origin: pos,
                    last: pos,
                });
            }
            egui::TouchPhase::Move
                if swipe
                    .as_ref()
                    .is_some_and(|active| active.device == device && active.id == id) =>
            {
                let active = swipe.as_mut().expect("matched active swipe");
                *platform_offset += pos.x - active.last.x;
                active.last = pos;
            }
            egui::TouchPhase::End
                if swipe
                    .as_ref()
                    .is_some_and(|active| active.device == device && active.id == id) =>
            {
                if let Some(active) = swipe.take() {
                    *platform_offset += pos.x - active.last.x;
                    let delta = pos - active.origin;
                    if delta.x.abs() >= SWIPE_THRESHOLD && delta.x.abs() > delta.y.abs() * 1.25 {
                        if delta.x < 0.0 {
                            let _stepped = step_panel(first, last_start, 1);
                        } else {
                            let _stepped = step_panel(first, last_start, -1);
                        }
                        ui.ctx().request_repaint();
                    }
                }
            }
            egui::TouchPhase::Cancel
                if swipe
                    .as_ref()
                    .is_some_and(|active| active.device == device && active.id == id) =>
            {
                *swipe = None;
            }
            egui::TouchPhase::Start
            | egui::TouchPhase::Move
            | egui::TouchPhase::End
            | egui::TouchPhase::Cancel => {}
        }
    }
}

fn navigation_row<L, C, R>(
    ui: &mut egui::Ui,
    id: &'static str,
    left: impl FnOnce(&mut egui::Ui) -> L,
    center: impl FnOnce(&mut egui::Ui) -> C,
    right: impl FnOnce(&mut egui::Ui) -> R,
) -> (L, C, R) {
    let side = chrome::MechanismSize::Large.side();
    let (rect, _response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), side), egui::Sense::hover());
    let mut left_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt((id, "left"))
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let left = left(&mut left_ui);
    let center_rect = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(side));
    let mut center_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt((id, "center"))
            .max_rect(center_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let center = center(&mut center_ui);
    let mut right_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt((id, "right"))
            .max_rect(rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let right = right(&mut right_ui);
    (left, center, right)
}

fn step_panel(first: &mut usize, last_start: usize, displacement: isize) -> bool {
    let prior = *first;
    *first = first.saturating_add_signed(displacement).min(last_start);
    *first != prior
}

impl WeatherApp {
    pub fn pulse(&mut self, ui: &mut egui::Ui) {
        self.absorb_events(ui.ctx());
        self.absorb_persistence();

        let mut inspector = std::mem::take(&mut self.mobile_inspector);
        let undo_enabled = self.map_undo.has_reversal_for(&self.active_view);
        let inspector_response =
            inspector.show(ui, undo_enabled, |ui, panel| self.mobile_panel(ui, panel));
        self.mobile_inspector = inspector;
        self.water.begin(brass_poolrooms::water::Domain::basin(
            inspector_response.domain,
        ));
        self.water
            .surge(ui.ctx(), inspector_response.platform_offset);
        if self.session_state.mobile_panel != Some(inspector_response.panel) {
            self.session_state.mobile_panel = Some(inspector_response.panel);
            self.mark_dirty();
        }
        for control in &inspector_response.controls {
            self.water.monoglyph(control);
        }
        if inspector_response.undo_clicked {
            self.undo_map_object();
        }

        let _center = egui::CentralPanel::default().show(ui, |ui| self.map(ui));
        let mut guide = std::mem::take(&mut self.guide);
        guide.show_mobile(ui.ctx(), &MOBILE_GUIDE_GROUPS);
        self.guide = guide;
        self.show_mobile_settings(ui.ctx());
    }

    fn mobile_panel(&mut self, ui: &mut egui::Ui, panel: usize) {
        match panel {
            0 => self.mobile_application_panel(ui),
            1 => self.mobile_field_panel(ui),
            2 => self.forecast_controls(ui),
            3 => self.active_view_panel(ui),
            4 => self.view_library_panel(ui),
            5 => {
                let _status = ui.label(chrome::muted(&self.status));
                ui.add_space(3.0);
                let _source = ui.label(chrome::muted(format!(
                    "FORECAST · NOAA\nMAP · {}",
                    self.basemap_status
                )));
            }
            _ => unreachable!("mobile Inspector admitted an unknown panel"),
        }
    }

    fn mobile_application_panel(&mut self, ui: &mut egui::Ui) {
        let _header = ApplicationHeader::new("HRRR").show(
            ui,
            &mut self.guide,
            &mut self.settings,
            &mut self.water,
        );
        ui.add_space(8.0);
        let _identity = ui.label(chrome::muted(
            "HIGH-RESOLUTION RAPID REFRESH\nNOAA FORECAST FIELD VIEWER",
        ));
        ui.add_space(8.0);
        let _navigation = ui.label(chrome::muted(
            "SWIPE LEFT OR RIGHT FOR THE NEXT PANEL. SCROLL VERTICALLY WITHIN A PANEL. TAP THE MAP TO PROBE; HOLD TO PIN.",
        ));
    }

    fn show_mobile_settings(&mut self, ctx: &egui::Context) {
        let mut water_effects = self.configuration.water_effects;
        let mut changed = false;
        self.settings
            .show_managed(ctx, &mut self.water, |settings| {
                settings.group("PRESENTATION");
                changed |= settings.boolean(MOBILE_WATER_EFFECTS, &mut water_effects);
            });
        if !changed {
            return;
        }
        self.configuration.water_effects = water_effects;
        self.water.set_wetness(if water_effects {
            Wetness::Wet
        } else {
            Wetness::Dry
        });
        if water_effects {
            "water effects enabled · battery use will increase"
        } else {
            "water effects disabled"
        }
        .clone_into(&mut self.status);
        self.mark_configuration_dirty();
    }

    fn mobile_field_panel(&mut self, ui: &mut egui::Ui) {
        let mut chosen_product = None;
        for &row in Product::ROWS {
            let _row = ui.horizontal(|ui| {
                let spacing = ui.spacing().item_spacing.x * row.len().saturating_sub(1) as f32;
                let width = (ui.available_width() - spacing) / row.len() as f32;
                for &product in row {
                    let response = ui.add_sized(
                        [width, 44.0],
                        egui::Button::new(product.label())
                            .selected(self.session_state.overlay.active() == Some(product)),
                    );
                    chrome::tension(ui, &response);
                    if response.hovered() {
                        self.water.hover(("mobile-product", product), response.rect);
                    }
                    if response.clicked() {
                        chosen_product = Some((product, response.rect));
                    }
                }
            });
        }
        if let Some((product, rect)) = chosen_product {
            self.water.select(rect);
            self.strike_overlay(product);
        }
    }
}
