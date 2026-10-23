//! The handheld projection on the desktop testbed: the Inspector is a
//! Drawer, fields are chosen from its Field panel, and its arrows page and
//! lower it.

use egui_tester::Result;
use hrrr_choreography::{Act, Field, Panel};

use crate::{harness::Harness, observation::shows, projection};

pub fn run(harness: &Harness<'_>) -> Result<()> {
    let app = harness.launch_handheld()?;
    let mut story = harness.story(&app)?;
    // A cold boot selects the smoke field, so the Drawer opens on the
    // forecast panel; page back to the field panel to choose another.
    let _initial = story.wait(shows::field(Field::SurfaceSmoke))?;
    let _forecast_panel = story.anchor(hrrr_contract::Target::Panel(Panel::Forecast))?;
    for act in [
        Act::PreviousPanel(Panel::Field),
        Act::SelectField(Field::CloudCover),
        Act::NextPanel(Panel::Forecast),
        Act::LowerInspector,
        Act::RaiseInspector,
    ] {
        projection::enact(&mut story, &app, act)?;
    }
    app.terminate()
}
