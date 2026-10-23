//! The presentation rail: a maintained score performed against the specimen
//! world, developing stills and optionally a film. Never a verdict.

use std::path::{Path, PathBuf};

use egui_demo::Showcase;
use egui_tester::{Result, demand};
use hrrr_choreography::{Score, perform};

use crate::{
    harness::Harness,
    projection::{Desk, Fence},
};

/// The product window the portfolio frame assumes.
const WINDOW: (u32, u32) = (1440, 920);

/// What one presentation take develops.
pub struct Take {
    pub score: Score,
    pub darkroom: PathBuf,
    pub film: bool,
}

/// The operator's installed basemap, by the XDG data-home law HRRR follows.
pub fn specimen_basemap() -> Result<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")))
        .ok_or_else(|| egui_tester::Error::Verdict {
            detail: "neither XDG_DATA_HOME nor HOME locates the specimen basemap".to_owned(),
        })?;
    let basemap = data.join("hrrr/basemap/basemap.pmtiles");
    demand(
        basemap.is_file(),
        format!("specimen basemap {} is not installed", basemap.display()),
    )?;
    Ok(basemap)
}

pub fn perform_take(harness: &Harness<'_>, take: &Take) -> Result<()> {
    let showcase = if take.film {
        Showcase::film(&take.darkroom, &format!("hrrr-{}", take.score.name()))?
    } else {
        Showcase::stills(&take.darkroom)?
    };
    let app = harness.launch(true)?;
    let mut story = harness.observed_story(&app, showcase)?;
    let frame = story.capture()?;
    demand(
        (frame.width(), frame.height()) == WINDOW,
        format!(
            "HRRR window is {}×{}, not the portfolio's {}×{}",
            frame.width(),
            frame.height(),
            WINDOW.0,
            WINDOW.1
        ),
    )?;
    perform(&mut Desk::new(&mut story, &app, Fence::Paint), take.score)?;
    let showcase = story.finish()?;
    app.terminate()?;
    for artifact in showcase.publish()? {
        println!("{}", artifact.display());
    }
    Ok(())
}
