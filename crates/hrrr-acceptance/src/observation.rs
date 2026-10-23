use egui_tester::Condition;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Observation {
    pub contract: String,
    pub launch: String,
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

impl Observation {
    /// Every saved view, rooted or shelved.
    pub fn view_count(&self) -> usize {
        self.cabinet.views.len()
            + self
                .cabinet
                .folders
                .iter()
                .map(|folder| folder.views.len())
                .sum::<usize>()
    }

    /// Whether any root or folder holds this saved view.
    pub fn holds_view(&self, name: &str) -> bool {
        self.cabinet.views.iter().any(|view| view == name)
            || self
                .cabinet
                .folders
                .iter()
                .any(|folder| folder.views.iter().any(|view| view == name))
    }
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub open: bool,
    pub fault: bool,
}

/// Presentation facts the product settles while laying out each frame.
#[derive(Debug, Deserialize)]
pub struct Poise {
    pub dock: Option<Dock>,
    pub drawer: Option<Drawer>,
    pub navigating: bool,
    pub painted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub struct Dock {
    pub raised: bool,
    pub resting: bool,
}

#[derive(Debug, Deserialize)]
pub struct Drawer {
    pub raised: bool,
    pub settled: bool,
    pub panel: String,
}

#[derive(Debug, Deserialize)]
pub struct Run {
    pub cycle: String,
    pub tether: String,
}

#[derive(Debug, Deserialize)]
pub struct Cabinet {
    pub views: Vec<String>,
    pub folders: Vec<Folder>,
}

#[derive(Debug, Deserialize)]
pub struct Folder {
    pub name: String,
    pub open: bool,
    pub views: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Viewport {
    pub center: [f64; 2],
    pub zoom: f64,
}

pub mod shows {
    use super::*;

    pub fn field(name: &'static str) -> Condition<Observation> {
        Condition::new(
            format!("{name} field selected"),
            move |state: &Observation| state.active_field.as_deref() == Some(name),
        )
    }

    pub fn pins(count: usize) -> Condition<Observation> {
        Condition::new(
            format!("{count} persistent pin(s)"),
            move |state: &Observation| state.pins.len() == count,
        )
    }

    pub fn dragging(slot: Option<usize>) -> Condition<Observation> {
        Condition::new(format!("pin drag {slot:?}"), move |state: &Observation| {
            state.dragging_pin == slot
        })
    }

    pub fn pin(slot: usize, point: [f64; 2]) -> Condition<Observation> {
        Condition::new(
            format!("pin {slot} at {point:?}"),
            move |state: &Observation| {
                state
                    .pins
                    .get(slot)
                    .is_some_and(|actual| near(*actual, point))
            },
        )
    }
}

pub fn near(left: [f64; 2], right: [f64; 2]) -> bool {
    (left[0] - right[0]).abs() <= 1.0e-10 && (left[1] - right[1]).abs() <= 1.0e-10
}
