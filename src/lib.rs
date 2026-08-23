use anyhow::Result;

mod air_quality;
mod app;
mod application_paths;
mod basemap;
mod basemap_artifact;
mod cache;
mod commands;
mod configuration;
mod decode;
mod host;
mod library;
mod library_ui;
mod map;
mod model;
mod persist;
mod source;
mod spec;
mod state;
mod tray;
mod vector_map;
mod view;
mod wind_barb;
mod witness;
mod worker;

/// Enter HRRR through its ordinary command-line boundary.
///
/// # Errors
///
/// Returns startup, command, storage, or native-host failures.
pub fn run() -> Result<()> {
    cleanse_relative_xdg()?;
    let mut arguments = std::env::args_os().skip(1);
    let Some(command) = arguments.next() else {
        return run_gui();
    };
    match command.to_str() {
        Some("--help" | "-h" | "help") => {
            print_help();
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("hrrr {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("basemap") => run_basemap(arguments),
        Some(other) => anyhow::bail!("unknown command `{other}`; run `hrrr --help`"),
        None => anyhow::bail!("command is not valid UTF-8"),
    }
}

/// Enter the native HRRR application directly.
///
/// # Errors
///
/// Returns native-host, storage, or application startup failures.
pub fn run_gui() -> Result<()> {
    let ctx = egui::Context::default();
    brass_poolrooms::chrome::install(&ctx);
    let trace = eternalist_apps::TraceGuard::arm()?;
    let result = host::run(ctx);
    trace.flush();
    result
}

fn run_basemap(mut arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<()> {
    let operation = arguments
        .next()
        .and_then(|argument| argument.into_string().ok())
        .unwrap_or_else(|| "status".to_owned());
    let paths = application_paths::ApplicationPaths::claim()?;
    match operation.as_str() {
        "install" => {
            let _instance = paths.lock_instance()?;
            let day = arguments.next();
            if arguments.next().is_some() {
                anyhow::bail!("usage: hrrr basemap install [YYYYMMDD]");
            }
            let day = day
                .as_deref()
                .map(|day| {
                    day.to_str()
                        .ok_or_else(|| anyhow::anyhow!("basemap date is not valid UTF-8"))
                })
                .transpose()?;
            let _installed = basemap_artifact::install(&paths, day)?;
            Ok(())
        }
        "status" if arguments.next().is_none() => basemap_artifact::status(&paths),
        "remove" if arguments.next().is_none() => {
            let _instance = paths.lock_instance()?;
            basemap_artifact::remove(&paths)
        }
        _ => anyhow::bail!(
            "unknown basemap operation `{operation}`; expected install, status, or remove"
        ),
    }
}

fn print_help() {
    println!(
        "\
HRRR native forecast-field viewer

Usage:
  hrrr
  hrrr basemap install [YYYYMMDD]
  hrrr basemap status
  hrrr basemap remove
  hrrr --help
  hrrr --version

The GUI reads NOAA forecast fields directly. `basemap install` explicitly
downloads the North American Protomaps core through zoom 11. Zoom 12 detail is
fetched only for visible tiles after the map enters that zoom and is retained
in the bounded disposable cache."
    );
}

fn cleanse_relative_xdg() -> Result<()> {
    const ROOTS: [&str; 5] = [
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
    ];
    let invalid = ROOTS
        .into_iter()
        .filter(|name| {
            std::env::var_os(name).is_some_and(|value| !std::path::Path::new(&value).is_absolute())
        })
        .collect::<Vec<_>>();
    if invalid.is_empty() {
        return Ok(());
    }
    reexec_without(&invalid)
}

#[cfg(unix)]
fn reexec_without(names: &[&str]) -> Result<()> {
    use std::os::unix::process::CommandExt as _;

    let executable = std::env::current_exe()?;
    let mut command = std::process::Command::new(executable);
    let _command = command.args(std::env::args_os().skip(1));
    for name in names {
        let _command = command.env_remove(name);
    }
    Err(command.exec().into())
}

#[cfg(not(unix))]
fn reexec_without(_names: &[&str]) -> Result<()> {
    Ok(())
}
