//! The map canvas: navigation, pins and the transient probe with their
//! plaques, and the field legend.

use super::*;

#[derive(Default)]
struct PinGesture {
    captured: bool,
    hot: Option<usize>,
}

#[derive(Clone, Copy, Default)]
struct MapTouch {
    gesture: Option<egui::MultiTouchInfo>,
    quarantined: bool,
}

#[derive(Clone, Copy)]
pub(super) struct PinTug {
    pub(super) slot: usize,
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

impl WeatherApp {
    pub(super) fn map(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        crate::witness::anchor(ui, Target::Map, response.rect);
        if self.disposition == Disposition::Dock {
            self.water.begin(Domain::shelf(rect));
        }
        let touch = self.map_touch(ui, rect);
        let pins = if touch.quarantined {
            self.cancel_pin_tug();
            PinGesture::default()
        } else {
            self.tug_pins(ui, rect)
        };
        let navigating = self.navigate(ui, &response, rect, pins.captured, touch);
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
        let finest = cover.finest_ready(|key| {
            self.tiles.contains(key)
                || self
                    .tile_rejections
                    .get(&key)
                    .is_some_and(|rejection| rejection.resolves())
        });
        #[cfg(feature = "egui-test")]
        let basemap_painted =
            finest.is_some_and(|stratum| stratum.intent == basemap::Intent::Required);
        let coherent = finest.map(|stratum| stratum.keys.clone());
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
            let warm_basemap = cover
                .strata
                .iter()
                .filter(|stratum| stratum.intent == basemap::Intent::Prefetch)
                .flat_map(|stratum| &stratum.keys)
                .filter_map(|key| self.tiles.get(*key).cloned())
                .collect::<Arc<[_]>>();
            let _basemap = painter.add(eternalist_apps::egui_wgpu::Callback::new_paint_callback(
                rect,
                VectorPaint {
                    tiles: self.presented_basemap.clone(),
                    center_world: self.viewport.center_mercator,
                    world_points: map::world_pixels(self.viewport) as f32,
                    viewport_points: [rect.width(), rect.height()],
                    view_zoom: self.viewport.zoom as f32,
                    apparition_span: basemap::APPARITION_SPAN,
                    settled: !navigating,
                    warm_tiles: warm_basemap,
                    repaint: ui.ctx().clone(),
                },
            ));
        }

        let painted_field = self.active_field().or_else(|| self.displayed_field.clone());
        let mut legend_scale = None;
        let paints_adaptive = painted_field
            .as_ref()
            .is_some_and(|(key, _field)| self.scales.adaptive(key.product));
        if !paints_adaptive {
            self.range_scale.arrest();
        }
        if let Some((key, field)) = painted_field {
            if self.scales.adaptive(key.product) {
                if self.range_survey.select(key.product) {
                    self.range_scale.reset();
                }
                if navigating {
                    self.range_scale.arrest();
                } else {
                    let peak = self.range_survey.discern(key, &field, self.viewport, rect);
                    let proposed =
                        self.scales
                            .reckon(key.product, self.range_scale.settled(), peak);
                    if let Some(repaint_after) = self.range_scale.observe(proposed, Instant::now())
                    {
                        ui.ctx().request_repaint_after(repaint_after);
                    }
                }
            }
            let scale = self.scale_for(key).clone();
            legend_scale = Some(scale.clone());
            let _field = painter.add(eternalist_apps::egui_wgpu::Callback::new_paint_callback(
                rect,
                map::FieldPaint {
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
        #[cfg(feature = "egui-test")]
        {
            self.poise.navigating = navigating;
            self.poise.painted = basemap_painted
                && (self.session_state.overlay.active().is_none() || self.active_field().is_some())
                && self.range_scale.latched();
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
        {
            if self.loading.is_some_and(|demand| {
                demand.intent == LoadIntent::Foreground(self.demand_id)
                    && Some(demand.key) == self.active_key()
            }) {
                self.water.show_loading(ui.ctx(), rect);
            } else {
                self.water.hide_loading();
            }
        }
        self.show_marks(ui.ctx(), &painter, rect, pins.hot);
    }

    fn navigate(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        pin_captured: bool,
        touch: MapTouch,
    ) -> bool {
        let before = self.viewport;
        let minimum_zoom = map::minimum_zoom(rect.height());
        self.viewport.zoom = self.viewport.zoom.max(minimum_zoom);
        if let Some(gesture) = touch.gesture {
            self.viewport = map::pinch_viewport(
                self.viewport,
                rect,
                gesture.center_pos,
                gesture.translation_delta,
                gesture.zoom_delta,
                minimum_zoom,
            );
            self.water.drag(rect, gesture.translation_delta.y);
        }
        let dragging = !touch.quarantined
            && !pin_captured
            && response.dragged_by(egui::PointerButton::Primary);
        if dragging {
            let delta = ui.input(|input| input.pointer.delta());
            let scale = map::world_pixels(self.viewport);
            self.viewport.center_mercator[0] -= f64::from(delta.x) / scale;
            self.viewport.center_mercator[1] -= f64::from(delta.y) / scale;
            self.water.drag(rect, delta.y);
        }
        let scroll = (!touch.quarantined).then(|| {
            ui.input(|input| {
                input
                    .pointer
                    .hover_pos()
                    .filter(|pointer| rect.contains(*pointer))
                    .map(|pointer| (input.smooth_scroll_delta.y, pointer))
            })
        });
        let scroll = scroll.flatten();
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
        let persistent_touch = response.long_touched();
        if !touch.quarantined
            && !pin_captured
            && persistent_touch
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.strike_point(rect, pointer, true);
            self.water.click(egui::Rect::from_center_size(
                pointer,
                egui::Vec2::splat(18.0),
            ));
        } else if !touch.quarantined
            && !pin_captured
            && response.secondary_clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.reap_pin_at(rect, pointer);
        } else if !touch.quarantined
            && !pin_captured
            && response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let persistent = ui.input(click_shifted);
            self.strike_point(rect, pointer, persistent);
            self.water.click(egui::Rect::from_center_size(
                pointer,
                egui::Vec2::splat(18.0),
            ));
        }
        touch.gesture.is_some() || dragging || scrolling
    }

    /// A multi-touch gesture over the map, quarantining single-pointer
    /// handling until every finger lifts.
    fn map_touch(&mut self, ui: &egui::Ui, rect: egui::Rect) -> MapTouch {
        let (gesture, any_touches) = ui.input(|input| (input.multi_touch(), input.any_touches()));
        let gesture = gesture.filter(|gesture| rect.contains(gesture.start_pos));
        if gesture.is_some() {
            self.map_multitouch = true;
        }
        let quarantined = self.map_multitouch;
        if self.map_multitouch && !any_touches {
            self.map_multitouch = false;
        }
        MapTouch {
            gesture,
            quarantined,
        }
    }

    fn cancel_pin_tug(&mut self) {
        let Some(tug) = self.pin_tug.take() else {
            return;
        };
        if let Some(pin) = self.pins.get_mut(tug.slot) {
            *pin = tug.origin;
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
            let hardware = chrome::ForgePin::new(anchor).size(chrome::MechanismSize::Medium);
            let response = ui.interact(
                hardware.grip(),
                egui::Id::new(("pin-bulb", slot)),
                egui::Sense::drag(),
            );
            crate::witness::anchor(ui, Target::Pin(slot), response.rect);
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
        if !persistent
            && self.capabilities.touch
            && self.transient_probe.is_some_and(|probe| {
                map::screen_at(self.viewport, rect, probe.world()).distance(pointer)
                    <= TRANSIENT_PROBE_REAP_RADIUS
            })
        {
            self.remember_probe();
            self.transient_probe = None;
            return;
        }
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
            let size = Self::plaque_size(ctx, id);
            let berth = phalanx.berth(crown, size, egui::vec2(11.0, 7.0));
            let response = self.point_popup(ctx, id, berth, pin, key, field.as_ref(), Some(slot));
            #[cfg(feature = "egui-test")]
            {
                self.poise.painted &= Self::plaque_ripe(ctx, id);
            }
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
                    Target::TransientProbe,
                    egui::Rect::from_center_size(anchor, egui::Vec2::splat(8.0)),
                );
                let id = egui::Id::new("transient-probe");
                let size = Self::plaque_size(ctx, id);
                let berth = phalanx.berth(anchor, size, egui::vec2(9.0, 9.0));
                let popup = self.point_popup(ctx, id, berth, probe, key, field.as_ref(), None);
                #[cfg(feature = "egui-test")]
                {
                    self.poise.painted &= Self::plaque_ripe(ctx, id);
                }
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

    /// A plaque's size from its previous pass; unknown during the sizing pass
    /// in which egui lays it out unseen.
    fn plaque_size(ctx: &egui::Context, id: egui::Id) -> egui::Vec2 {
        ctx.memory(|memory| memory.area_rect(id).map(|rect| rect.size()))
            .unwrap_or_default()
    }

    /// Whether a plaque this pass showed is sized and past egui's fade-in.
    #[cfg(feature = "egui-test")]
    fn plaque_ripe(ctx: &egui::Context, id: egui::Id) -> bool {
        egui::AreaState::load(ctx, id)
            .and_then(|area| area.size.and(area.last_became_visible_at))
            .is_some_and(|born| {
                ctx.input(|input| input.time - born) >= f64::from(ctx.global_style().animation_time)
            })
    }

    fn point_popup(
        &mut self,
        ctx: &egui::Context,
        id: egui::Id,
        berth: PlaqueBerth,
        point: MercatorPoint,
        key: Option<FrameKey>,
        field: Option<&(FrameKey, Arc<FieldGrid>)>,
        pin: Option<usize>,
    ) -> PlaqueResponse {
        let sample = field.and_then(|(_, field)| sample_point(point, field));
        let mut reap = false;
        let popup = egui::Area::new(id)
            .order(egui::Order::Foreground)
            .pivot(berth.pivot)
            .fixed_pos(berth.position)
            .constrain(false)
            .show(ctx, |ui| {
                let close = pin
                    .is_some()
                    .then(|| chrome::CornerClose::new().size(chrome::MechanismSize::Small));
                let margin = egui::Margin::symmetric(8, 6);
                let pane = egui::Frame::new()
                    .fill(chrome::SURFACE)
                    .stroke(egui::Stroke::new(1.0_f32, chrome::EDGE_STRONG))
                    .inner_margin(margin)
                    .show(ui, |ui| {
                        let valid = key
                            .map(|key| (key.run, key.product, key.valid))
                            .or_else(|| {
                                self.run
                                    .zip(self.session_state.overlay.active())
                                    .map(|(run, product)| (run, product, self.session_state.lead))
                            })
                            .map_or_else(
                                || "NO FORECAST".to_owned(),
                                |(run, product, lead)| {
                                    product
                                        .lead_axis(run)
                                        .and_then(|axis| axis.local_label(lead))
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
                        } else if self.session_state.overlay.active().is_some() {
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
                    if let Some(slot) = pin {
                        crate::witness::response(ui, Target::PinRemove(slot), &close);
                    }
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
        let font = chrome::spatial_font(painter.ctx(), FONT_SIZE, egui::FontFamily::Monospace);
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
}

/// Whether Shift held when the primary button released: a click's own
/// modifiers survive a pass that also delivers Shift's release.
fn click_shifted(input: &egui::InputState) -> bool {
    input
        .events
        .iter()
        .rev()
        .find_map(|event| match event {
            egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
                ..
            } => Some(modifiers.shift),
            _ => None,
        })
        .unwrap_or(input.modifiers.shift)
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;

    #[test]
    fn pin_tug_forges_one_reversal_only_for_net_motion() -> Result<()> {
        let origin = MercatorPoint::forge([0.2, 0.5]).context("tug origin")?;
        let moved = MercatorPoint::forge([0.3, 0.5]).context("tug destination")?;
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
}
