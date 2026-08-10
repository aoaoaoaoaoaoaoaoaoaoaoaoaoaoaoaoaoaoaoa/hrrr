use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use egui_tester::{
    AppCommand, Application, Graphics, Network, ReactionBudget, Result, Story, Testbed, WindowQuery,
};

use crate::observation::Observation;

pub const TITLE: &str = "HRRR · native forecast fields";
pub const VIEWS: &str = "xdg/data/hrrr/views.toml";

pub type HrrrStory<'app, 'bed> = Story<'app, 'bed, Observation>;

pub struct Harness<'a> {
    pub testbed: &'a Testbed,
    pub binary: &'a Path,
    pub artifacts: Option<&'a Path>,
}

impl<'a> Harness<'a> {
    pub const fn new(testbed: &'a Testbed, binary: &'a Path, artifacts: Option<&'a Path>) -> Self {
        Self {
            testbed,
            binary,
            artifacts,
        }
    }

    pub fn command(&self, witnessed: bool) -> AppCommand {
        let command = AppCommand::new(self.binary)
            .private_env("HRRR_BASEMAP_ARCHIVE", "fixtures/basemap.pmtiles")
            .graphics(Graphics::Software)
            .network(Network::Deny)
            .runtime(Duration::from_secs(45));
        if witnessed {
            command.witness("probes/hrrr.observations")
        } else {
            command
        }
    }

    pub fn launch(&self, witnessed: bool) -> Result<Application<'a>> {
        self.testbed.launch(self.command(witnessed))
    }

    pub fn story<'app>(&'a self, app: &'app Application<'a>) -> Result<HrrrStory<'app, 'a>> {
        let mut story: HrrrStory<'app, 'a> = Story::bind(
            self.testbed,
            app,
            WindowQuery::title_exact(TITLE),
            ReactionBudget::functional(Duration::from_secs(5)),
        )?;
        let ready = story.ready(Duration::from_secs(15))?;
        egui_tester::demand(
            ready.state.contract == hrrr_contract::UI_FINGERPRINT,
            format!(
                "HRRR UI contract mismatch: expected {}, observed {}",
                hrrr_contract::UI_FINGERPRINT,
                ready.state.contract
            ),
        )?;
        egui_tester::demand(
            ready.state.launch == "ready",
            format!("HRRR stopped at launch phase `{}`", ready.state.launch),
        )?;
        Ok(story)
    }
}

pub fn sibling_binary() -> Result<PathBuf> {
    let executable = std::env::current_exe().map_err(|source| egui_tester::Error::Io {
        operation: "resolve acceptance executable",
        path: PathBuf::from("<current executable>"),
        source,
    })?;
    executable
        .parent()
        .map(|parent| parent.join("hrrr"))
        .ok_or_else(|| egui_tester::Error::Verdict {
            detail: "acceptance executable has no sibling directory".to_owned(),
        })
}
