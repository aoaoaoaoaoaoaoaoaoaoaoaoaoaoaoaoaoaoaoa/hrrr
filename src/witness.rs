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
        pub launch: &'static str,
        pub active_field: Option<String>,
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

    impl State {
        pub fn threshold(launch: &'static str) -> Self {
            Self {
                contract: hrrr_contract::UI_FINGERPRINT,
                launch,
                active_field: None,
                active_view: String::new(),
                pins: Vec::new(),
                transient_probe: None,
                dragging_pin: None,
                viewport: Viewport {
                    center: [0.0; 2],
                    zoom: 0.0,
                },
            }
        }
    }
}
