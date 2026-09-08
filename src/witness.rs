pub use eternalist_apps::witness::{anchor, rect, response};

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
        pub lead_hour: u8,
        pub base_hour: Option<u8>,
        pub active_view: String,
        pub pins: Vec<[f64; 2]>,
        pub transient_probe: Option<[f64; 2]>,
        pub dragging_pin: Option<usize>,
        pub guide_open: bool,
        pub close_to_tray: bool,
        pub settings: Settings,
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
                lead_hour: 0,
                base_hour: None,
                active_view: String::new(),
                pins: Vec::new(),
                transient_probe: None,
                dragging_pin: None,
                guide_open: false,
                close_to_tray: false,
                settings: Settings::default(),
                viewport: Viewport {
                    center: [0.0; 2],
                    zoom: 0.0,
                },
            }
        }
    }

    #[derive(Default, Serialize)]
    pub struct Settings {
        pub open: bool,
        pub fault: bool,
    }
}
