//! The handheld projection on the desktop testbed: the Inspector is a
//! Drawer, fields are chosen from its Field panel, and its arrows page and
//! lower it.

use std::time::Duration;

use egui_tester::{Button, Result, demand};

use crate::{
    harness::{Harness, HrrrStory},
    observation::shows,
};

const CLOUD_COVER: &str = "cloud-cover";
/// Wire names of the Drawer's arrows, as Apps' witness publishes them.
const DRAWER_NEXT: &str = "eternalist.drawer.next";
const DRAWER_PREVIOUS: &str = "eternalist.drawer.previous";
const DRAWER_LOWER: &str = "eternalist.drawer.lower";
const DRAWER_RAISE: &str = "eternalist.drawer.raise";

pub fn run(harness: &Harness<'_>) -> Result<()> {
    let app = harness.launch_handheld()?;
    let mut story = harness.story(&app)?;
    // A cold boot selects the smoke field, so the Drawer opens on the
    // forecast panel; page back to the field panel to choose another.
    let _initial = story.wait(shows::field("smoke"))?;
    let _forecast_panel = story.anchor(hrrr_contract::Target::Panel("forecast"))?;
    let previous = at_rest(&mut story, DRAWER_PREVIOUS)?;
    let _reaction = story.click_at(previous, Button::Primary)?;
    let _field_panel = story.anchor(hrrr_contract::Target::Panel("field"))?;
    let cloud_cover = story.anchor(hrrr_contract::Target::Field(CLOUD_COVER))?;
    let selected = story
        .click_at(cloud_cover.center(), Button::Primary)?
        .until(shows::field(CLOUD_COVER))?
        .into_value();
    demand(
        selected.state.active_field.as_deref() == Some(CLOUD_COVER),
        "the Drawer's field panel did not select cloud cover",
    )?;

    let next = at_rest(&mut story, DRAWER_NEXT)?;
    let _reaction = story.click_at(next, Button::Primary)?;
    let _forecast_again = story.anchor(hrrr_contract::Target::Panel("forecast"))?;

    let lower = at_rest(&mut story, DRAWER_LOWER)?;
    let _reaction = story.click_at(lower, Button::Primary)?;
    let raise = at_rest(&mut story, DRAWER_RAISE)?;
    let _reaction = story.click_at(raise, Button::Primary)?;
    let _raised = story.anchor(DRAWER_LOWER)?;
    app.terminate()
}

/// The Drawer slides when it pages, lowers, or raises; an arrow's rectangle
/// read mid-slide would send the click to the map beneath.
fn at_rest(story: &mut HrrrStory<'_, '_>, name: &str) -> Result<(i16, i16)> {
    let frame = story.wait_stable(
        Duration::from_secs(8),
        Duration::from_millis(400),
        format!("{name} at rest"),
        |frame| {
            frame
                .anchors
                .iter()
                .find(|anchor| anchor.name == name)
                .map(|anchor| anchor.rect)
        },
    )?;
    let anchor = frame
        .anchors
        .iter()
        .find(|anchor| anchor.name == name)
        .ok_or_else(|| egui_tester::Error::Verdict {
            detail: format!("{name} vanished after settling"),
        })?;
    Ok(anchor.center())
}
