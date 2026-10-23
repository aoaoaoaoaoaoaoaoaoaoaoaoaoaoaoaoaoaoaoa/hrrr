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
        pub poise: Poise,
        pub run: Option<Run>,
        pub cabinet: Cabinet,
    }

    /// Presentation facts settled while a frame is laid out.
    #[derive(Clone, Copy, Default, Serialize)]
    pub struct Poise {
        pub dock: Option<Dock>,
        pub drawer: Option<Drawer>,
        /// A pointer, wheel, or touch gesture is still moving the map.
        pub navigating: bool,
        /// The active field, its latched scale, the required basemap stratum,
        /// and every visible plaque, sized and faded in, are on the surface.
        pub painted: bool,
    }

    /// The Inspector's Dock: its target state and whether its slide reached it.
    #[derive(Clone, Copy, Serialize)]
    pub struct Dock {
        pub raised: bool,
        pub resting: bool,
    }

    /// The handheld Inspector: its height, whether it rests, and its Panel.
    #[derive(Clone, Copy, Serialize)]
    pub struct Drawer {
        pub raised: bool,
        pub settled: bool,
        pub panel: &'static str,
    }

    #[derive(Serialize)]
    pub struct Run {
        /// Cycle stamp, `YYYYMMDDHH` in UTC.
        pub cycle: String,
        pub tether: &'static str,
    }

    #[derive(Default, Serialize)]
    pub struct Cabinet {
        pub views: Vec<String>,
        pub folders: Vec<Folder>,
    }

    #[derive(Serialize)]
    pub struct Folder {
        pub name: String,
        pub open: bool,
        pub views: Vec<String>,
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
                poise: Poise::default(),
                run: None,
                cabinet: Cabinet::default(),
            }
        }
    }

    #[derive(Default, Serialize)]
    pub struct Settings {
        pub open: bool,
        pub fault: bool,
    }
}
