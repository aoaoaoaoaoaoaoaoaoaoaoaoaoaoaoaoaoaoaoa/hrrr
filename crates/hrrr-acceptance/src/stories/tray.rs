use std::time::Duration;

use egui_tester::{AppCommand, Button, Frame, Network, Result, WindowQuery, demand};

use crate::harness::{Harness, TITLE};

const TRAY: &str = "HRRR tray";
const MENU: &str = "HRRR tray menu";

pub fn run(harness: &Harness<'_>) -> Result<()> {
    let _config = harness.testbed.create_private_dir("xdg/config/hrrr")?;
    let _preferences = harness
        .testbed
        .write_private("xdg/config/hrrr/config.toml", b"close_minimizes = true\n")?;
    let manager = harness.testbed.launch(
        AppCommand::new("/usr/bin/i3")
            .args(["-c", "/test/fixtures/i3.config"])
            .network(Network::Deny)
            .runtime(Duration::from_secs(45)),
    )?;
    let controller = harness.testbed.x11()?;
    manager.wait_until(Duration::from_secs(10), "i3bar to claim the screen", || {
        Ok(controller.find_window("i3bar")?.is_some())
    })?;

    let app = harness.launch(false)?;
    let main = controller.wait_window_query(
        &app,
        WindowQuery::title_exact(TITLE),
        Duration::from_secs(15),
    )?;
    let tray = controller.wait_window_query(
        &app,
        WindowQuery::title_exact(TRAY),
        Duration::from_secs(10),
    )?;
    controller.focus(&main)?;
    let _close = controller.close(&main)?;
    app.wait_until(
        Duration::from_secs(5),
        "window to hide into the tray",
        || {
            Ok(controller
                .find_windows(&WindowQuery::title_exact(TITLE))?
                .is_empty())
        },
    )?;
    let _reveal = controller.click(&tray, 8, 8, Button::Primary)?;
    let restored = controller.wait_window_query(
        &app,
        WindowQuery::title_exact(TITLE),
        Duration::from_secs(5),
    )?;
    let restored_session = harness.testbed.x11_session(
        &app,
        WindowQuery::title_exact(TITLE),
        Duration::from_secs(2),
    )?;
    let first = restored_session.capture()?;
    let frame = if visible(&first) {
        first
    } else {
        restored_session.wait_changed(&first, 0.001, 2, Duration::from_secs(5))?
    };
    demand(
        visible(&frame),
        "tray reveal mapped a window without product pixels",
    )?;
    if let Some(artifacts) = harness.artifacts {
        frame.save_png(artifacts.join("tray-restored.png"))?;
    }

    controller.focus(&restored)?;
    let _close = restored_session.close()?;
    app.wait_until(
        Duration::from_secs(5),
        "window to hide before tray quit",
        || {
            Ok(controller
                .find_windows(&WindowQuery::title_exact(TITLE))?
                .is_empty())
        },
    )?;
    let _menu = controller.click(&tray, 8, 8, Button::Secondary)?;
    let menu = controller.wait_window_query(
        &app,
        WindowQuery::title_exact(MENU),
        Duration::from_secs(5),
    )?;
    if let Some(artifacts) = harness.artifacts {
        controller
            .capture(&menu)?
            .save_png(artifacts.join("tray-menu.png"))?;
    }
    let _quit = controller.click(&menu, 52, 15, Button::Primary)?;
    let exit = app.wait(Duration::from_secs(5))?;
    demand(
        exit.success(),
        format!("tray quit failed: {}; {}", exit.result, exit.stderr),
    )?;
    app.terminate()?;
    manager.terminate()
}

fn visible(frame: &Frame) -> bool {
    let pixels = frame.rgba().chunks_exact(4);
    let total = pixels.len();
    let painted = pixels.filter(|pixel| pixel[..3] != [0, 0, 0]).count();
    painted > total / 4
}
