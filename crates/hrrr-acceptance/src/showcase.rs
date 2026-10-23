//! The presentation rail: the showcase score performed against the specimen
//! world, developing its stills and optionally a film. Never a verdict.

use std::{
    ffi::OsString,
    os::unix::ffi::OsStringExt as _,
    path::{Path, PathBuf},
    process::Command,
};

use egui_demo::Showcase;
use egui_tester::{ArtifactStem, Error, Result, demand};
use hrrr_choreography::{Score, perform};

use crate::{
    harness::Harness,
    projection::{Desk, Fence},
};

/// The film's stem.
const FILM: ArtifactStem = ArtifactStem::literal("hrrr-showcase");

/// A showcase take: its stills alone, or its stills and its film.
pub enum Take {
    Stills(PathBuf),
    Film(PathBuf),
}

impl Take {
    fn showcase(&self) -> Result<Showcase> {
        match self {
            Self::Stills(directory) => Showcase::stills(directory),
            Self::Film(directory) => Showcase::film(directory, &FILM),
        }
    }
}

/// The operator's basemap archive, as the product itself resolves it.
pub fn specimen_basemap(binary: &Path) -> Result<PathBuf> {
    let resolved = Command::new(binary)
        .args(["basemap", "path"])
        .output()
        .map_err(|source| Error::Io {
            operation: "ask HRRR for its basemap archive",
            path: binary.to_owned(),
            source,
        })?;
    demand(
        resolved.status.success(),
        format!(
            "`hrrr basemap path` failed: {}",
            String::from_utf8_lossy(&resolved.stderr).trim()
        ),
    )?;
    let mut raw = resolved.stdout;
    if raw.last() == Some(&b'\n') {
        let _newline = raw.pop();
    }
    let basemap = PathBuf::from(OsString::from_vec(raw));
    demand(
        basemap.is_file(),
        format!("specimen basemap {} is not installed", basemap.display()),
    )?;
    Ok(basemap)
}

pub fn perform_take(harness: &Harness<'_>, take: &Take) -> Result<()> {
    let app = harness.launch(true)?;
    let mut story = harness.observed_story(&app, take.showcase()?)?;
    let mut desk = Desk::new(&mut story, &app, Fence::Paint);
    desk.painted()?;
    perform(&mut desk, Score::Showcase)?;
    let showcase = story.finish()?;
    app.terminate()?;
    for artifact in showcase.publish()? {
        println!("{}", artifact.display());
    }
    Ok(())
}
