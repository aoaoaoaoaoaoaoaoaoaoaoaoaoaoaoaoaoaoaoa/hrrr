use anyhow::Result;

mod app;
mod basemap;
mod basemap_artifact;
mod cache;
mod config;
mod decode;
mod fold_ui;
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
mod witness;
mod worker;
mod xdg;

fn main() -> Result<()> {
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

fn run_gui() -> Result<()> {
    let ctx = egui::Context::default();
    dwemer_poolrooms::chrome::install(&ctx);
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
    let lair = xdg::Lair::claim()?;
    match operation.as_str() {
        "install" => {
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
            let _installed = basemap_artifact::install(&lair, day)?;
            Ok(())
        }
        "status" if arguments.next().is_none() => basemap_artifact::status(&lair),
        "remove" if arguments.next().is_none() => basemap_artifact::remove(&lair),
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

The GUI reads NOAA HRRR forecast fields directly. `basemap install` explicitly
downloads the North American Protomaps basemap through zoom 12."
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
