//! Android's bottom Inspector and touch projection.

use super::*;

const PANELS: [&str; 6] = [
    "application",
    "field",
    "forecast",
    "active view",
    "views",
    "status",
];
const PANEL_WIDTH: f32 = 340.0;
const COLLAPSED_HEIGHT: f32 = 42.0;
const MIN_EXPANDED_HEIGHT: f32 = 220.0;
const SWIPE_THRESHOLD: f32 = 64.0;

#[derive(Debug)]
pub(super) struct MobileInspector {
    first: usize,
    expanded: bool,
    swipe: Option<(egui::TouchDeviceId, egui::TouchId, egui::Pos2)>,
}

impl Default for MobileInspector {
    fn default() -> Self {
        Self {
            first: 0,
            expanded: true,
            swipe: None,
        }
    }
}

impl MobileInspector {
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        mut add: impl FnMut(&mut egui::Ui, usize),
    ) -> Vec<chrome::MonoglyphResponse> {
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
        let first = &mut self.first;
        let swipe = &mut self.swipe;
        let mut controls = Vec::with_capacity(3);
        let _panel = egui::Panel::show_switched(
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
                        columns,
                        last_start,
                        &mut requested_expansion,
                        &mut controls,
                        &mut add,
                    );
                } else {
                    let (_title, raise, _hint) = navigation_row(
                        ui,
                        "collapsed",
                        |ui| ui.label(chrome::section_title(PANELS[*first].to_uppercase())),
                        |ui| {
                            chrome::Monoglyph::symbol(chrome::Symbol::ArrowUp)
                                .show(ui)
                                .on_hover_text("Raise the Inspector")
                        },
                        |ui| ui.label(chrome::muted("SWIPE PANELS")),
                    );
                    if raise.clicked() {
                        requested_expansion = Some(true);
                    }
                    controls.push(raise);
                }
            },
        );
        self.expanded = requested_expansion.unwrap_or(expanded_state);
        if self.expanded != expanded_state {
            ui.ctx().request_repaint();
        }
        controls
    }
}

fn show_expanded(
    ui: &mut egui::Ui,
    first: &mut usize,
    swipe: &mut Option<(egui::TouchDeviceId, egui::TouchId, egui::Pos2)>,
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
            PANELS[*first].to_uppercase(),
            *first + 1,
            PANELS.len()
        )
    } else {
        format!(
            "{} · {}–{}/{}",
            PANELS[*first].to_uppercase(),
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
        step_panel(first, last_start, -1);
    }
    if next.clicked() {
        step_panel(first, last_start, 1);
    }
    if lower.clicked() {
        *requested_expansion = Some(false);
    }
    controls.extend([previous, lower, next]);

    let body = ui.available_rect_before_wrap();
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
    for (device, id, phase, pos) in touches {
        match phase {
            egui::TouchPhase::Start if body.contains(pos) && swipe.is_none() => {
                *swipe = Some((device, id, pos));
            }
            egui::TouchPhase::End
                if swipe.is_some_and(|(active_device, active_id, _)| {
                    active_device == device && active_id == id
                }) =>
            {
                if let Some((_, _, origin)) = swipe.take() {
                    let delta = pos - origin;
                    if delta.x.abs() >= SWIPE_THRESHOLD && delta.x.abs() > delta.y.abs() * 1.25 {
                        if delta.x < 0.0 {
                            step_panel(first, last_start, 1);
                        } else {
                            step_panel(first, last_start, -1);
                        }
                        ui.ctx().request_repaint();
                    }
                }
            }
            egui::TouchPhase::Cancel
                if swipe.is_some_and(|(active_device, active_id, _)| {
                    active_device == device && active_id == id
                }) =>
            {
                *swipe = None;
            }
            egui::TouchPhase::Start
            | egui::TouchPhase::Move
            | egui::TouchPhase::End
            | egui::TouchPhase::Cancel => {}
        }
    }

    ui.columns(columns, |columns_ui| {
        for (column, panel_ui) in columns_ui.iter_mut().enumerate() {
            let panel = *first + column;
            let _title =
                panel_ui.label(chrome::section_title(PANELS[panel].to_uppercase()).size(16.0));
            let _separator = panel_ui.separator();
            let _scroll = egui::ScrollArea::vertical()
                .id_salt(("mobile-inspector-panel", panel))
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .auto_shrink([false, false])
                .show(panel_ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui, panel);
                });
        }
    });
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

fn step_panel(first: &mut usize, last_start: usize, displacement: isize) {
    *first = first.saturating_add_signed(displacement).min(last_start);
}

impl WeatherApp {
    pub fn pulse(&mut self, ui: &mut egui::Ui) {
        self.absorb_events(ui.ctx());
        self.absorb_persistence();
        let settings_invoked = self.settings.take_shortcut(ui.ctx());
        let guide_invoked = self.guide.take_shortcuts(ui.ctx());
        if !settings_invoked
            && !guide_invoked
            && let Some(dispatch) =
                commands::canon().route(ui.ctx(), &[], |edict| self.edict_status(edict))
        {
            self.apply_edict(dispatch);
        }
        self.take_keys(ui.ctx());

        let mut inspector = std::mem::take(&mut self.mobile_inspector);
        let controls = inspector.show(ui, |ui, panel| self.mobile_panel(ui, panel));
        self.mobile_inspector = inspector;
        for control in &controls {
            self.water.monoglyph(control);
        }

        let _center = egui::CentralPanel::default().show(ui, |ui| self.map(ui));
        let mut guide = std::mem::take(&mut self.guide);
        guide.show(
            ui.ctx(),
            commands::canon(),
            &[],
            |()| "FORECAST",
            |edict| self.edict_status(edict),
            &GUIDE_GROUPS,
        );
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
            "SWIPE LEFT OR RIGHT FOR THE NEXT PANEL. SCROLL VERTICALLY WITHIN A PANEL.",
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
