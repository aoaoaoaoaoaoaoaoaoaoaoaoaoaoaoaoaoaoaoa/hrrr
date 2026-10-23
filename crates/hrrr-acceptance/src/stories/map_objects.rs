use std::time::Duration;

use egui_tester::{Condition, Result, Wheel, demand};
use hrrr_choreography::Act;
use serde::Deserialize;

use crate::{
    harness::{Harness, VIEWS},
    observation::{Observation, near, shows},
    projection,
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
    projection::enact(&mut story, &app, Act::PlaceProbe)?;
    demand(
        story.frame()?.state.pins.is_empty(),
        "ordinary click forged a persistent pin",
    )?;
    projection::enact(&mut story, &app, Act::UndoMapChange)?;
    let _transient_undone = story.wait(Condition::new(
        "transient probe undone",
        |state: &Observation| state.transient_probe.is_none(),
    ))?;
    projection::enact(&mut story, &app, Act::PlacePin)?;
    let origin = story.frame()?.state.pins[0];
    app.wait_until(Duration::from_secs(4), "pin to reach views.toml", || {
        Ok(persisted_view(harness)
            .is_some_and(|view| view.pins.first().is_some_and(|point| near(*point, origin))))
    })?;
    app.terminate()?;
    drop(story);
    drop(app);
    Ok(origin)
}

fn drag_undo_and_persist(harness: &Harness<'_>, origin: [f64; 2]) -> Result<f64> {
    let app = harness.launch(true)?;
    let mut story = harness.story(&app)?;
    let _restored = story.wait(shows::pin(0, origin))?;
    projection::enact(&mut story, &app, Act::DragPin)?;
    projection::enact(&mut story, &app, Act::UndoMapChange)?;
    let _undone = story.wait(shows::pin(0, origin))?;
    if let Some(artifacts) = harness.artifacts {
        story
            .capture()?
            .save_png(artifacts.join("map-objects.png"))?;
    }
    let map = story.anchor(hrrr_contract::Target::Map)?;
    let zoom_ceiling = batter_zoom_ceiling(&mut story, map.center())?;
    app.wait_until(
        Duration::from_secs(4),
        "undone pin and zoom ceiling to reach views.toml",
        || {
            Ok(persisted_view(harness).is_some_and(|view| {
                view.pins.first().is_some_and(|point| near(*point, origin))
                    && view.viewport.zoom.to_bits() == zoom_ceiling.to_bits()
            }))
        },
    )?;
    app.terminate()?;
    drop(story);
    drop(app);
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

fn persisted_view(harness: &Harness<'_>) -> Option<SavedView> {
    let raw = harness.testbed.read_private_to_string(VIEWS).ok()?;
    let views = toml::from_str::<ViewLibrary>(&raw).ok()?;
    views.saved.into_iter().next()
}

#[derive(Deserialize)]
struct ViewLibrary {
    saved: Vec<SavedView>,
}

#[derive(Deserialize)]
struct SavedView {
    viewport: SavedViewport,
    pins: Vec<[f64; 2]>,
}

#[derive(Deserialize)]
struct SavedViewport {
    zoom: f64,
}
