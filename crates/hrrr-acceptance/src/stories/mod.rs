mod map_objects;
mod tray;

use std::time::Duration;

use egui_tester::{Frame, Result, WindowQuery, demand};

use crate::harness::{Harness, TITLE};

pub fn smoke(harness: &Harness<'_>) -> Result<()> {
    let app = harness.launch(false)?;
    let session = harness.testbed.x11_session(
        &app,
        WindowQuery::title_exact(TITLE),
        Duration::from_secs(15),
    )?;
    session.focus()?;
    let first = session.capture()?;
    let visible = |frame: &Frame| {
        frame
            .rgba()
            .chunks_exact(4)
            .any(|pixel| pixel[..3] != [0, 0, 0])
    };
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

pub fn run(harness: &Harness<'_>, selected: Option<&str>) -> Result<()> {
    match selected.unwrap_or("map-objects") {
        "map-objects" => map_objects::run(harness)?,
        "tray" => tray::run(harness)?,
        unknown => {
            return Err(egui_tester::Error::Verdict {
                detail: format!("unknown HRRR story `{unknown}`; expected map-objects or tray"),
            });
        }
    }
    println!(
        "hrrr acceptance passed: {}",
        selected.unwrap_or("map-objects")
    );
    Ok(())
}
