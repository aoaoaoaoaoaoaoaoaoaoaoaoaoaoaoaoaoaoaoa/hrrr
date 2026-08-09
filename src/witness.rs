use std::fmt::Display;

use egui::{Rect, Ui};

#[inline]
pub fn anchor(ui: &Ui, name: impl Display, rect: Rect) {
    #[cfg(feature = "egui-test")]
    egui_tester_witness::egui::record(ui, name.to_string(), rect);
    #[cfg(not(feature = "egui-test"))]
    {
        let _ = (ui, rect);
        drop(name);
    }
}

#[inline]
pub fn rect(ctx: &egui::Context, name: impl Display, rect: Rect) {
    #[cfg(feature = "egui-test")]
    egui_tester_witness::egui::record_rect(ctx, name.to_string(), rect);
    #[cfg(not(feature = "egui-test"))]
    {
        let _ = (ctx, rect);
        drop(name);
    }
}

#[cfg(feature = "egui-test")]
pub use active::*;

#[cfg(feature = "egui-test")]
mod active {
    use serde::Serialize;

    #[derive(Serialize)]
    pub struct State {
        pub contract: &'static str,
        pub active_view: String,
        pub pins: Vec<[f64; 2]>,
        pub transient_probe: Option<[f64; 2]>,
        pub dragging_pin: Option<usize>,
        pub viewport: Viewport,
    }

    #[derive(Clone, Copy, Serialize)]
    pub struct Viewport {
        pub center: [f64; 2],
        pub zoom: f64,
    }
}
