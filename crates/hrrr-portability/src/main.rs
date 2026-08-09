use anyhow::{Context as _, Result, bail, ensure};
use egui_tester_witness::{Error as WitnessError, ObservationJournal};
use serde_json::Value;
use std::{
    fs::File,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

const STARTUP_LIMIT: Duration = Duration::from_secs(45);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const TRAY_FAILURE: &str = "could not raise HRRR tray icon";

fn main() -> Result<()> {
    let binary = binary()?;
    prove_cli(&binary)?;

    let cell = Cell::forge()?;
    let basemap = cell.path().join("basemap.pmtiles");
    let _archive = File::create(&basemap).context("forge inert basemap archive")?;
    let witness = cell.path().join("hrrr.observations");
    let frames = cell.path().join("hrrr.frames");
    let launch = format!(
        "hrrr-portability-{}-{}",
        std::env::consts::OS,
        std::process::id()
    );

    let mut command = Command::new(&binary);
    let _command = command
        .env("HRRR_BASEMAP_ARCHIVE", &basemap)
        .env("EGUI_TESTER_WITNESS", &witness)
        .env("EGUI_TESTER_FRAMES", &frames)
        .env("EGUI_TESTER_LAUNCH", &launch)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_host_paths(&mut command, cell.path())?;

    let child = command
        .spawn()
        .with_context(|| format!("launch {}", binary.display()))?;
    let mut captive = Captive::new(child);
    let verdict = await_presented_frame(captive.child_mut()?, &witness, &launch)?;
    let output = captive.finish()?;
    std::fs::write(cell.path().join("hrrr.stdout"), &output.stdout)
        .context("retain HRRR portability stdout")?;
    std::fs::write(cell.path().join("hrrr.stderr"), &output.stderr)
        .context("retain HRRR portability stderr")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    match verdict {
        Startup::Ready => {}
        Startup::Exited(status) => bail!(
            "HRRR exited before its first witnessed frame with {status}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        ),
        Startup::TimedOut => bail!(
            "HRRR presented no witnessed frame within {STARTUP_LIMIT:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        ),
    }
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        ensure!(
            !stderr.contains(TRAY_FAILURE),
            "native tray construction failed\nstderr:\n{stderr}"
        );
    }
    println!(
        "HRRR portability passed: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    Ok(())
}

struct Cell {
    path: PathBuf,
    _temporary: Option<tempfile::TempDir>,
}

impl Cell {
    fn forge() -> Result<Self> {
        if let Some(path) = std::env::var_os("HRRR_PORTABILITY_ARTIFACTS") {
            let path = PathBuf::from(path).join(format!(
                "{}-{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH,
                std::process::id()
            ));
            std::fs::create_dir_all(&path)
                .with_context(|| format!("create portability artifacts at {}", path.display()))?;
            return Ok(Self {
                path,
                _temporary: None,
            });
        }
        let temporary = tempfile::tempdir().context("forge portability cell")?;
        Ok(Self {
            path: temporary.path().to_path_buf(),
            _temporary: Some(temporary),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

fn binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HRRR_PORTABILITY_BINARY") {
        return Ok(PathBuf::from(path));
    }
    let executable = std::env::current_exe().context("resolve portability executable")?;
    let parent = executable
        .parent()
        .context("portability executable has no parent")?;
    Ok(parent.join(format!("hrrr{}", std::env::consts::EXE_SUFFIX)))
}

fn prove_cli(binary: &Path) -> Result<()> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("run {} --version", binary.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure!(
        output.status.success(),
        "{} --version failed with {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        binary.display(),
        output.status
    );
    ensure!(
        stdout.trim().starts_with("hrrr "),
        "{} --version returned an alien identity: {stdout:?}",
        binary.display()
    );
    Ok(())
}

fn isolate_host_paths(command: &mut Command, root: &Path) -> Result<()> {
    let home = root.join("home");
    // `SHGetKnownFolderPath` expands the profile-relative AppData defaults
    // after observing USERPROFILE. Keep those known folders inside the cell;
    // free-standing APPDATA directories leave the Windows shell resolver with
    // a profile whose canonical children do not exist.
    let roaming = home.join("AppData").join("Roaming");
    let local = home.join("AppData").join("Local");
    let roots = [
        ("HOME", home.clone()),
        ("USERPROFILE", home),
        ("APPDATA", roaming),
        ("LOCALAPPDATA", local),
        ("XDG_CACHE_HOME", root.join("cache")),
        ("XDG_CONFIG_HOME", root.join("config")),
        ("XDG_DATA_HOME", root.join("data")),
        ("XDG_STATE_HOME", root.join("state")),
        ("XDG_RUNTIME_DIR", root.join("runtime")),
    ];
    for (name, path) in roots {
        std::fs::create_dir_all(&path)
            .with_context(|| format!("create isolated {name} at {}", path.display()))?;
        let _command = command.env(name, path);
    }
    Ok(())
}

enum Startup {
    Ready,
    Exited(ExitStatus),
    TimedOut,
}

fn await_presented_frame(child: &mut Child, path: &Path, launch: &str) -> Result<Startup> {
    let begun = Instant::now();
    let mut journal = ObservationJournal::sealed(path, launch);
    while begun.elapsed() < STARTUP_LIMIT {
        if let Some(status) = child.try_wait().context("poll HRRR process")? {
            return Ok(Startup::Exited(status));
        }
        match journal.read_new::<Value>() {
            Ok(frames) => {
                for frame in frames {
                    if presented_hrrr_frame(&frame)? {
                        return Ok(Startup::Ready);
                    }
                }
            }
            Err(WitnessError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("read HRRR witness"),
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(Startup::TimedOut)
}

fn presented_hrrr_frame(frame: &Value) -> Result<bool> {
    let Some(state) = frame.get("state") else {
        return Ok(false);
    };
    let contract = state
        .get("contract")
        .and_then(Value::as_str)
        .context("witness state has no contract")?;
    ensure!(
        contract == hrrr_contract::UI_FINGERPRINT,
        "HRRR UI contract mismatch: expected {}, observed {contract}",
        hrrr_contract::UI_FINGERPRINT
    );
    let presented = frame
        .get("surface_sequence")
        .and_then(Value::as_u64)
        .is_some_and(|sequence| sequence > 0);
    let map = frame
        .get("anchors")
        .and_then(Value::as_array)
        .is_some_and(|anchors| {
            anchors.iter().any(|anchor| {
                anchor.get("name").and_then(Value::as_str)
                    == Some(&hrrr_contract::Target::Map.to_string())
            })
        });
    Ok(presented && map)
}

struct Captive(Option<Child>);

impl Captive {
    const fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn child_mut(&mut self) -> Result<&mut Child> {
        self.0.as_mut().context("HRRR child was already reaped")
    }

    fn finish(mut self) -> Result<Output> {
        let mut child = self.0.take().context("HRRR child was already reaped")?;
        if child
            .try_wait()
            .context("poll HRRR before teardown")?
            .is_none()
        {
            child
                .kill()
                .context("terminate HRRR after portability proof")?;
        }
        child.wait_with_output().context("reap HRRR process")
    }
}

impl Drop for Captive {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _killed = child.kill();
            let _reaped = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn readiness_requires_contract_surface_and_map() -> Result<()> {
        let ready = json!({
            "surface_sequence": 1,
            "anchors": [{ "name": "map.canvas", "rect": [0.0, 0.0, 1.0, 1.0] }],
            "state": { "contract": hrrr_contract::UI_FINGERPRINT },
        });
        assert!(presented_hrrr_frame(&ready)?);
        assert!(!presented_hrrr_frame(&json!({
            "surface_sequence": 0,
            "anchors": [{ "name": "map.canvas" }],
            "state": { "contract": hrrr_contract::UI_FINGERPRINT },
        }))?);
        Ok(())
    }

    #[test]
    fn alien_contract_is_fatal() {
        let alien = json!({
            "surface_sequence": 1,
            "anchors": [{ "name": "map.canvas" }],
            "state": { "contract": "hrrr.ui/alien" },
        });
        assert!(presented_hrrr_frame(&alien).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_known_folders_remain_inside_the_isolated_profile() -> Result<()> {
        let cell = tempfile::tempdir()?;
        let mut command = Command::new("hrrr.exe");
        isolate_host_paths(&mut command, cell.path())?;
        let environment = command
            .get_envs()
            .filter_map(|(name, value)| value.map(|value| (name, value)))
            .collect::<std::collections::BTreeMap<_, _>>();
        let profile = Path::new(environment[std::ffi::OsStr::new("USERPROFILE")]);
        for variable in ["APPDATA", "LOCALAPPDATA"] {
            let path = Path::new(environment[std::ffi::OsStr::new(variable)]);
            assert!(path.starts_with(profile), "{variable} escaped USERPROFILE");
            assert!(path.is_dir(), "{variable} was not forged");
        }
        Ok(())
    }
}
