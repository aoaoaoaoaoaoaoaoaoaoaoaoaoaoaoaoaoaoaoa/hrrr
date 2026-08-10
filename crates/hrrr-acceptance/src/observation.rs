use egui_tester::Condition;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Observation {
    pub contract: String,
    pub launch: String,
    pub active_field: Option<String>,
    pub active_view: String,
    pub pins: Vec<[f64; 2]>,
    pub transient_probe: Option<[f64; 2]>,
    pub dragging_pin: Option<usize>,
    pub viewport: Viewport,
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
