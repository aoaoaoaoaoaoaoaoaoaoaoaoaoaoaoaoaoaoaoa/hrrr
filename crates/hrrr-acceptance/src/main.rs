mod fixture;
mod harness;
mod observation;
mod projection;
mod showcase;
mod stories;

use std::{env, path::PathBuf};

use egui_tester::{Backend, Error, Result, TestbedBuilder, WaylandConfig, X11Config};
use hrrr_choreography::Score;

use crate::{
    harness::{Harness, World},
    showcase::Take,
};

fn main() -> Result<()> {
    let cli = Cli::parse()?;
    let binary = env::var_os("HRRR_ACCEPTANCE_BINARY")
        .map(PathBuf::from)
        .map_or_else(harness::sibling_binary, Ok)?;
    let artifacts = cli
        .artifacts
        .or_else(|| env::var_os("HRRR_ACCEPTANCE_ARTIFACTS").map(PathBuf::from));
    let mut builder = TestbedBuilder::default().backend(cli.backend);
    if let Some(artifacts) = &artifacts {
        builder = builder.failure_artifacts(artifacts);
    }
    builder.run(|testbed| match &cli.rail {
        Rail::Showcase { score, take } => {
            let world = World::Specimen {
                basemap: showcase::specimen_basemap(&binary)?,
            };
            showcase::perform_take(
                &Harness::new(testbed, &binary, artifacts.as_deref(), world),
                *score,
                take,
            )
        }
        Rail::Acceptance(proof) => {
            let _fixtures = fixture::FixtureWorld::raise(testbed)?;
            let harness = Harness::new(testbed, &binary, artifacts.as_deref(), World::Fixture);
            match proof {
                Proof::Smoke => stories::smoke(&harness, cli.backend),
                Proof::Story(story) => stories::run(&harness, story.as_deref()),
            }
        }
    })
}

/// One execution's rail: hermetic acceptance or the presentation take.
enum Rail {
    Acceptance(Proof),
    Showcase { score: Score, take: Take },
}

enum Proof {
    Smoke,
    Story(Option<String>),
}

struct Cli {
    rail: Rail,
    artifacts: Option<PathBuf>,
    backend: Backend,
}

impl Cli {
    fn parse() -> Result<Self> {
        let mut args = env::args_os().skip(1);
        let mut story = None;
        let mut artifacts = None;
        let mut smoke = false;
        let mut score = None;
        let mut take = None;
        let mut backend = Backend::X11(X11Config::default());
        while let Some(argument) = args.next() {
            match argument.to_str() {
                Some("--story") => {
                    story = Some(
                        required(&mut args, "--story")?
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
                Some("--artifacts") => {
                    artifacts = Some(PathBuf::from(required(&mut args, "--artifacts")?));
                }
                Some("--smoke") => smoke = true,
                Some(flag @ ("--stills" | "--film")) => {
                    if take.is_some() {
                        return Err(verdict("a showcase is one take: --stills or --film"));
                    }
                    let directory = PathBuf::from(required(&mut args, flag)?);
                    take = Some(if flag == "--film" {
                        Take::Film(directory)
                    } else {
                        Take::Stills(directory)
                    });
                }
                Some("--score") => {
                    let value = required(&mut args, "--score")?;
                    score = Some(Score::parse(
                        value
                            .to_str()
                            .ok_or_else(|| verdict("--score must be valid Unicode"))?,
                    )?);
                }
                Some("--backend") => {
                    let value = required(&mut args, "--backend")?;
                    backend = match value.to_str() {
                        Some("x11") => Backend::X11(X11Config::default()),
                        Some("wayland") => Backend::Wayland(WaylandConfig::default()),
                        Some(value) => {
                            return Err(verdict(format!(
                                "unknown acceptance backend `{value}`; expected x11 or wayland"
                            )));
                        }
                        None => return Err(verdict("acceptance backend must be valid Unicode")),
                    };
                }
                Some(flag) => return Err(verdict(format!("unknown acceptance option `{flag}`"))),
                None => return Err(verdict("acceptance options must be valid Unicode")),
            }
        }
        let rail = match (smoke, story, take, score) {
            (false, story, None, None) => Rail::Acceptance(Proof::Story(story)),
            (true, None, None, None) => Rail::Acceptance(Proof::Smoke),
            (false, None, Some(take), score) => Rail::Showcase {
                score: score.unwrap_or(Score::Showcase),
                take,
            },
            (_, _, None, Some(_)) => {
                return Err(verdict("--score performs only with --stills or --film"));
            }
            _ => {
                return Err(verdict(
                    "--smoke, --story, and --stills or --film select exclusive rails",
                ));
            }
        };
        if matches!(backend, Backend::Wayland(_)) && !matches!(rail, Rail::Acceptance(Proof::Smoke))
        {
            return Err(verdict(
                "Wayland admits launch-and-capture smoke only; native stories require X11",
            ));
        }
        Ok(Self {
            rail,
            artifacts,
            backend,
        })
    }
}

fn required(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    flag: &str,
) -> Result<std::ffi::OsString> {
    args.next()
        .ok_or_else(|| verdict(format!("{flag} requires a value")))
}

fn verdict(detail: impl Into<String>) -> Error {
    Error::Verdict {
        detail: detail.into(),
    }
}
