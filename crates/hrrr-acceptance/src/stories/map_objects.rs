use std::time::Duration;

use egui_tester::{Button, Condition, Key, Modifiers, Motion, Result, Wheel, demand};
use serde::Deserialize;

use crate::{
    harness::{Harness, VIEWS},
    observation::{Observation, near, shows},
};

pub fn run(harness: &Harness<'_>) -> Result<()> {
    let origin = place_and_persist(harness)?;
    let zoom_ceiling = drag_undo_and_persist(harness, origin)?;
    prove_final_restart(harness, origin, zoom_ceiling)
}

fn place_and_persist(harness: &Harness<'_>) -> Result<[f64; 2]> {
    let app = harness.launch(true)?;
    let mut story = harness.story(&app)?;
    let initial = story.wait(shows::pins(0))?;
    demand(
        initial.state.active_view == "default",
        "cold boot did not select default",
    )?;
    let map = story.anchor(hrrr_contract::Target::Map)?;
    let transient_at = (map.center().0.saturating_sub(120), map.center().1);
    let transient = story
        .click_at(transient_at, Button::Primary)?
        .until(Condition::new(
            "transient probe placed",
            |state: &Observation| state.transient_probe.is_some(),
        ))?
        .into_value();
    demand(
        transient.state.pins.is_empty(),
        "ordinary click forged a persistent pin",
    )?;
    let _transient_undone =
        story
            .chord(Modifiers::CTRL, Key::Character('z'))?
            .until(Condition::new(
                "transient probe undone",
                |state: &Observation| state.transient_probe.is_none(),
            ))?;
    let shift = story.session().key_down(Key::Shift)?;
    let _shifted = story.reaction(shift).next_frame()?;
    let (x, y) = map.center();
    let click = story.session().click(x, y, Button::Primary)?;
    let placed = story.reaction(click).until(shows::pins(1))?.into_value();
    let release = story.session().key_up(Key::Shift)?;
    let _released = story.reaction(release).next_frame()?;
    let origin = placed.state.pins[0];
    let _settled = story.wait_stable(
        Duration::from_secs(4),
        Duration::from_millis(700),
        "persistent pin to survive the autosave interval",
        |frame| (frame.state.pins.as_slice() == [origin]).then_some(()),
    )?;
    app.terminate()?;
    drop(story);
    drop(app);
    demand(
        persisted_pin(harness, 0).is_some_and(|point| near(point, origin)),
        "persistent pin witness advanced before views.toml",
    )?;
    Ok(origin)
}

fn drag_undo_and_persist(harness: &Harness<'_>, origin: [f64; 2]) -> Result<f64> {
    let app = harness.launch(true)?;
    let mut story = harness.story(&app)?;
    let _restored = story.wait(shows::pin(0, origin))?;
    let pin = story.anchor(hrrr_contract::Target::Pin(0))?;
    let from = pin.center();
    let to = (from.0.saturating_add(96), from.1.saturating_sub(48));
    let press = story
        .session()
        .button_down(from.0, from.1, Button::Primary)?;
    let _pressed = story.reaction(press).next_frame()?;
    let moved = story
        .motion_to(to, Motion::default())?
        .until(
            shows::dragging(Some(0))
                & Condition::new("pin 0 moved", move |state: &Observation| {
                    state
                        .pins
                        .first()
                        .is_some_and(|point| !near(*point, origin))
                }),
        )?
        .into_value();
    let displaced = moved.state.pins[0];
    let release = story.session().button_up(Button::Primary)?;
    let _released = story
        .reaction(release)
        .until(shows::dragging(None) & shows::pin(0, displaced))?;
    let _undone = story
        .chord(Modifiers::CTRL, Key::Character('z'))?
        .until(shows::pin(0, origin))?;
    let _settled = story.wait_stable(
        Duration::from_secs(4),
        Duration::from_millis(700),
        "undone pin drag to survive the autosave interval",
        |frame| (frame.state.pins.as_slice() == [origin]).then_some(()),
    )?;
    if let Some(artifacts) = harness.artifacts {
        story
            .capture()?
            .save_png(artifacts.join("map-objects.png"))?;
    }
    let map = story.anchor(hrrr_contract::Target::Map)?;
    let zoom_ceiling = batter_zoom_ceiling(&mut story, map.center())?;
    let _settled = story.wait_stable(
        Duration::from_secs(4),
        Duration::from_millis(700),
        "zoom ceiling to survive the autosave interval",
        |frame| (frame.state.viewport.zoom.to_bits() == zoom_ceiling.to_bits()).then_some(()),
    )?;
    app.terminate()?;
    drop(story);
    drop(app);
    demand(
        persisted_pin(harness, 0).is_some_and(|point| near(point, origin)),
        "undo restored the witness but not views.toml",
    )?;
    Ok(zoom_ceiling)
}

fn batter_zoom_ceiling(
    story: &mut crate::harness::HrrrStory<'_, '_>,
    center: (i16, i16),
) -> Result<f64> {
    let wheel = Wheel {
        tick_duration: Duration::from_millis(2),
    };
    let zoomed = story.wheel(center, -64, wheel)?.next_frame()?.into_value();
    let ceiling = zoomed.state.viewport.zoom;
    demand(
        (16.0..=17.0).contains(&ceiling),
        format!("rapid wheel gesture stopped at implausible zoom {ceiling}"),
    )?;
    let battered = story.wheel(center, -64, wheel)?.next_frame()?.into_value();
    demand(
        battered.state.viewport.zoom.to_bits() == ceiling.to_bits(),
        format!(
            "continued wheel input breached zoom ceiling {ceiling} with {}",
            battered.state.viewport.zoom
        ),
    )?;
    Ok(ceiling)
}

fn prove_final_restart(harness: &Harness<'_>, origin: [f64; 2], zoom_ceiling: f64) -> Result<()> {
    let app = harness.launch(true)?;
    let mut story = harness.story(&app)?;
    let restored = story.wait(shows::pin(0, origin))?;
    demand(
        restored.state.transient_probe.is_none(),
        "restart resurrected a transient probe",
    )?;
    demand(
        restored.state.viewport.zoom.to_bits() == zoom_ceiling.to_bits()
            && restored
                .state
                .viewport
                .center
                .into_iter()
                .all(f64::is_finite),
        "restart lost or corrupted the saturated viewport",
    )?;
    app.terminate()
}

fn persisted_pin(harness: &Harness<'_>, slot: usize) -> Option<[f64; 2]> {
    let raw = harness.testbed.read_private_to_string(VIEWS).ok()?;
    let views = toml::from_str::<ViewLibrary>(&raw).ok()?;
    views.saved.first()?.pins.get(slot).copied()
}

#[derive(Deserialize)]
struct ViewLibrary {
    saved: Vec<SavedView>,
}

#[derive(Deserialize)]
struct SavedView {
    pins: Vec<[f64; 2]>,
}
