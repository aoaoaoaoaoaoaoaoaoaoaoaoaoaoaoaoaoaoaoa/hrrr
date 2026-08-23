use std::time::Duration;

use egui_tester::{Button, Condition, Result, demand};

use crate::{
    harness::{Harness, SESSION_STATE},
    observation::{Observation, shows},
};

const CLOUD_COVER: &str = "cloud-cover";

pub fn run(harness: &Harness<'_>) -> Result<()> {
    select_cloud_cover(harness)?;
    prove_restart(harness)?;
    prove_cumulative_interval(harness)
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
        |frame| (frame.state.active_field.as_deref() == Some(CLOUD_COVER)).then_some(()),
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

fn prove_cumulative_interval(harness: &Harness<'_>) -> Result<()> {
    let _state = harness.testbed.create_private_dir("xdg/state/hrrr")?;
    let _slate = harness.testbed.write_private(
        SESSION_STATE,
        b"schema = 2\n\
overlay = \"qpf_run\"\n\
cycle = \"fixed\"\n\
fixed_run = 1785272400\n\
lead = 12\n\
base = 3\n\
closed_folders = []\n",
    )?;
    let app = harness.launch(true)?;
    let mut story = harness.story(&app)?;
    let _initial = story.wait(Condition::new(
        "F03–F12 cumulative interval",
        |state: &Observation| {
            state.active_field.as_deref() == Some("qpf")
                && state.lead_hour == 12
                && state.base_hour == Some(3)
        },
    ))?;
    let base = story.anchor(hrrr_contract::Target::BaseHour)?;
    let base_six = base.rect[0] + (base.rect[2] - base.rect[0]) * (6.0 / 18.0);
    let _base_moved = story
        .click_at((base_six.round() as i16, base.center().1), Button::Primary)?
        .until(Condition::new(
            "fixed-lattice base advanced to F06",
            |state: &Observation| state.base_hour == Some(6) && state.lead_hour == 12,
        ))?;

    let valid = story.anchor(hrrr_contract::Target::ForecastHour)?;
    let left = ((valid.rect[0] + 1.0).round() as i16, valid.center().1);
    let _valid_locked = story
        .click_at(left, Button::Primary)?
        .until(Condition::new(
            "valid hour stopped one hour beyond base",
            |state: &Observation| state.base_hour == Some(6) && state.lead_hour == 7,
        ))?;
    let _settled = story.wait_stable(
        Duration::from_secs(4),
        Duration::from_millis(700),
        "cumulative interval to survive the autosave interval",
        |frame| (frame.state.base_hour == Some(6) && frame.state.lead_hour == 7).then_some(()),
    )?;
    if let Some(artifacts) = harness.artifacts {
        story
            .capture()?
            .save_png(artifacts.join("cumulative-interval.png"))?;
    }
    app.terminate()?;
    drop(story);
    drop(app);

    let app = harness.launch(true)?;
    let mut story = harness.story(&app)?;
    let _restored = story.wait(Condition::new(
        "restored F06–F07 cumulative interval",
        |state: &Observation| state.base_hour == Some(6) && state.lead_hour == 7,
    ))?;
    app.terminate()
}
