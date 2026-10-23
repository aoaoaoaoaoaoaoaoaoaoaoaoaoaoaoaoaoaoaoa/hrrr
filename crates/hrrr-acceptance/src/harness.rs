use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use egui_tester::{
    AppCommand, Application, Graphics, Network, ReactionBudget, Result, Silent, Story,
    StoryObserver, Testbed, WindowQuery,
};

use crate::observation::Observation;

pub const TITLE: &str = "HRRR";
pub const SESSION_STATE: &str = "xdg/state/hrrr/slate.toml";
pub const VIEWS: &str = "xdg/data/hrrr/views.toml";

pub type HrrrStory<'app, 'bed, O = Silent> = Story<'app, 'bed, Observation, O>;

/// The data an HRRR launch sees.
pub enum World {
    /// Hermetic fixtures: a forged basemap, a pinned publication frontier,
    /// denied network, and deterministic software graphics.
    Fixture,
    /// The operator's basemap borrowed read-only, live NOAA forecasts of the
    /// newest finished run pinned for the whole take and cached privately,
    /// and host graphics. Presentation only; never a verdict.
    Specimen { basemap: PathBuf },
}

pub struct Harness<'a> {
    pub testbed: &'a Testbed,
    pub binary: &'a Path,
    pub artifacts: Option<&'a Path>,
    pub world: World,
}

impl<'a> Harness<'a> {
    pub const fn new(
        testbed: &'a Testbed,
        binary: &'a Path,
        artifacts: Option<&'a Path>,
        world: World,
    ) -> Self {
        Self {
            testbed,
            binary,
            artifacts,
            world,
        }
    }

    pub fn command(&self, witnessed: bool) -> AppCommand {
        let command = AppCommand::new(self.binary);
        let command = match &self.world {
            World::Fixture => command
                .private_env("HRRR_BASEMAP_ARCHIVE", "fixtures/basemap.pmtiles")
                .env("HRRR_ACCEPTANCE_PUBLISHED", "18")
                .graphics(Graphics::Software)
                .network(Network::Deny)
                .runtime(Duration::from_secs(45)),
            World::Specimen { basemap } => command
                .args(["--run", "finished"])
                .env("HRRR_BASEMAP_ARCHIVE", basemap)
                .borrow_read_only(basemap)
                .graphics(Graphics::Host)
                .network(Network::Host)
                .runtime(Duration::from_mins(10)),
        };
        if witnessed {
            command.witness("probes/hrrr.observations")
        } else {
            command
        }
    }

    pub fn launch(&self, witnessed: bool) -> Result<Application<'a>> {
        self.testbed.launch(self.command(witnessed))
    }

    /// Launch HRRR as a handheld would: no pointer, touch, no keyboard, no
    /// free GPU budget, no configuration file, retirement.
    pub fn launch_handheld(&self) -> Result<Application<'a>> {
        self.testbed.launch(
            self.command(true)
                .env("ETERNALIST_CAPABILITIES", "handheld"),
        )
    }

    pub fn story<'app>(&'a self, app: &'app Application<'a>) -> Result<HrrrStory<'app, 'a>> {
        self.observed_story(app, Silent)
    }

    /// Bind a story whose causal stream reaches `observer` from the first frame.
    pub fn observed_story<'app, O: StoryObserver>(
        &'a self,
        app: &'app Application<'a>,
        observer: O,
    ) -> Result<HrrrStory<'app, 'a, O>> {
        let mut story = HrrrStory::bind(
            self.testbed,
            app,
            WindowQuery::title_exact(TITLE),
            ReactionBudget::functional(Duration::from_secs(5)),
        )?
        .with_observer(observer)?;
        let ready = story.ready(Duration::from_secs(45))?;
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
