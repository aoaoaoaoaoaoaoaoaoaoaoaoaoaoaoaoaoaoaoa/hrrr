use std::time::Duration;

use egui_tester::{Button, Result, demand};

use crate::{harness::Harness, observation::shows};

const CLOUD_COVER: &str = "cloud-cover";

pub fn run(harness: &Harness<'_>) -> Result<()> {
    select_cloud_cover(harness)?;
    prove_restart(harness)
}

fn select_cloud_cover(harness: &Harness<'_>) -> Result<()> {
    let app = harness.launch(true)?;
    let mut story = harness.story(&app)?;
    let initial = story.wait(shows::field("smoke"))?;
    demand(
        initial.state.active_field.as_deref() == Some("smoke"),
        "cold boot did not select the default smoke field",
    )?;
    let cloud_cover = story.anchor(hrrr_contract::Target::Field(CLOUD_COVER))?;
    let selected = story
        .click_at(cloud_cover.center(), Button::Primary)?
        .until(shows::field(CLOUD_COVER))?
        .into_value();
    demand(
        selected.state.active_field.as_deref() == Some(CLOUD_COVER),
        "cloud-cover control did not select cloud cover",
    )?;
    let _settled = story.wait_stable(
        Duration::from_secs(4),
        Duration::from_millis(700),
        "cloud-cover selection to survive the autosave interval",
        |frame| {
            (frame.state.active_field.as_deref() == Some(CLOUD_COVER)).then_some(())
        },
    )?;
    app.terminate()
}

fn prove_restart(harness: &Harness<'_>) -> Result<()> {
    let app = harness.launch(true)?;
    let mut story = harness.story(&app)?;
    let restored = story.wait(shows::field(CLOUD_COVER))?;
    demand(
        restored.state.active_field.as_deref() == Some(CLOUD_COVER),
        "restart did not restore cloud cover",
    )?;
    app.terminate()
}
