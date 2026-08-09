mod fixture;
mod harness;
mod observation;
mod stories;

use std::{env, path::PathBuf};

use egui_tester::{Error, Result, TestbedBuilder};

fn main() -> Result<()> {
    let cli = Cli::parse()?;
    let binary = env::var_os("HRRR_ACCEPTANCE_BINARY")
        .map(PathBuf::from)
        .map_or_else(harness::sibling_binary, Ok)?;
    let artifacts = cli
        .artifacts
        .or_else(|| env::var_os("HRRR_ACCEPTANCE_ARTIFACTS").map(PathBuf::from));
    let mut builder = TestbedBuilder::default();
    if let Some(artifacts) = &artifacts {
        builder = builder.failure_artifacts(artifacts);
    }
    builder.run(|testbed| {
        let _fixtures = fixture::FixtureWorld::raise(testbed)?;
        let harness = harness::Harness::new(testbed, &binary, artifacts.as_deref());
        if cli.smoke {
            stories::smoke(&harness)
        } else {
            stories::run(&harness, cli.story.as_deref())
        }
    })
}

struct Cli {
    story: Option<String>,
    artifacts: Option<PathBuf>,
    smoke: bool,
}

impl Cli {
    fn parse() -> Result<Self> {
        let mut args = env::args_os().skip(1);
        let mut story = None;
        let mut artifacts = None;
        let mut smoke = false;
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
                Some(flag) => return Err(verdict(format!("unknown acceptance option `{flag}`"))),
                None => return Err(verdict("acceptance options must be valid Unicode")),
            }
        }
        if smoke && story.is_some() {
            return Err(verdict("--smoke and --story are mutually exclusive"));
        }
        Ok(Self {
            story,
            artifacts,
            smoke,
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
