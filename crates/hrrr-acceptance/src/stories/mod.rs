mod fields;
mod keyboard;
mod map_objects;
mod tray;

use std::time::Duration;

use egui_tester::{Backend, Frame, Result, WindowQuery, demand};

use crate::{
    harness::{Harness, TITLE},
    observation::Observation,
};

pub fn smoke(harness: &Harness<'_>, backend: Backend) -> Result<()> {
    match backend {
        Backend::X11(_) => smoke_x11(harness),
        Backend::Wayland(_) => smoke_wayland(harness),
    }
}

fn smoke_x11(harness: &Harness<'_>) -> Result<()> {
    let app = harness.launch(false)?;
    let session = harness.testbed.x11_session(
        &app,
        WindowQuery::title_exact(TITLE),
        Duration::from_secs(15),
    )?;
    session.focus()?;
    let first = session.capture()?;
    let frame = if visible(&first) {
        first
    } else {
        session.wait_changed(&first, 0.001, 2, Duration::from_secs(15))?
    };
    demand(
        visible(&frame),
        "uninstrumented HRRR rendered only black pixels",
    )?;
    app.terminate()
}

fn smoke_wayland(harness: &Harness<'_>) -> Result<()> {
    let app = harness.launch(true)?;
    let mut witness = app.witness()?.typed::<Observation>();
    let presented = witness.wait_surface_presented(&app, Duration::from_secs(15))?;
    demand(
        presented.state.contract == hrrr_contract::UI_FINGERPRINT,
        format!(
            "HRRR UI contract mismatch: expected {}, observed {}",
            hrrr_contract::UI_FINGERPRINT,
            presented.state.contract
        ),
    )?;
    demand(
        presented.state.launch == "ready",
        format!(
            "Wayland HRRR stopped at launch phase `{}`",
            presented.state.launch
        ),
    )?;
    app.wait_until(
        Duration::from_secs(15),
        "nonblack pixels on the headless Wayland output",
        || Ok(visible(&harness.testbed.capture_wayland()?)),
    )?;
    app.terminate()
}

fn visible(frame: &Frame) -> bool {
    frame
        .rgba()
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[..3] != [0, 0, 0])
}

pub fn run(harness: &Harness<'_>, selected: Option<&str>) -> Result<()> {
    match selected.unwrap_or("map-objects") {
        "fields" => fields::run(harness)?,
        "keyboard" => keyboard::run(harness)?,
        "map-objects" => map_objects::run(harness)?,
        "tray" => tray::run(harness)?,
        unknown => {
            return Err(egui_tester::Error::Verdict {
                detail: format!(
                    "unknown HRRR story `{unknown}`; expected fields, keyboard, map-objects, or tray"
                ),
            });
        }
    }
    println!(
        "hrrr acceptance passed: {}",
        selected.unwrap_or("map-objects")
    );
    Ok(())
}
