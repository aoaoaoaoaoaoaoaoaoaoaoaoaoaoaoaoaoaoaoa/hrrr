use crate::{persist::save_toml, xdg::Lair};
use anyhow::{Context as _, Result, bail};
#[cfg(target_os = "linux")]
use flate2::read::GzDecoder;
use jiff::{Timestamp, civil::Date, tz::TimeZone};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::ffi::OsStr;
use std::{
    error::Error as StdError,
    fmt,
    fs::File,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::Duration,
};

pub const ARCHIVE_NAME: &str = "basemap.pmtiles";
pub const LOCAL_MAX_ZOOM: u8 = 11;
pub const MAX_ZOOM: u8 = 12;
pub const BOUNDS: [f64; 4] = [-135.0, 21.0, -60.0, 54.0];
const TOOL_VERSION: &str = "1.31.2";
const TOOL_ORIGIN: &str = "https://github.com/protomaps/go-pmtiles/releases/download";
const MAP_ORIGIN: &str = "https://build.protomaps.com";
static ARENA_NONCE: AtomicU64 = AtomicU64::new(0);
const CHILD_POLL: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallPhase {
    FetchingTool,
    CheckingTool,
    UnpackingTool,
    ExtractingMap,
    CheckingMap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstallProgress {
    pub phase: InstallPhase,
    pub bytes: u64,
    pub total: Option<u64>,
}

impl InstallProgress {
    const fn phase(phase: InstallPhase) -> Self {
        Self {
            phase,
            bytes: 0,
            total: None,
        }
    }

    const fn bytes(phase: InstallPhase, bytes: u64, total: Option<u64>) -> Self {
        Self {
            phase,
            bytes,
            total,
        }
    }
}

#[derive(Debug)]
struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("basemap installation canceled")
    }
}

impl StdError for Cancelled {}

#[derive(Clone, Copy)]
struct ToolAsset {
    name: &'static str,
    sha256: &'static str,
    executable: &'static str,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const TOOL_ASSET: Option<ToolAsset> = Some(ToolAsset {
    name: "go-pmtiles_1.31.2_Linux_x86_64.tar.gz",
    sha256: "3ed7dbf4ec2e6dfe5e25b6f70d1ffc932729f93c86db353bf514dd71010a312f",
    executable: "pmtiles",
});

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const TOOL_ASSET: Option<ToolAsset> = Some(ToolAsset {
    name: "go-pmtiles_1.31.2_Linux_arm64.tar.gz",
    sha256: "f8bd47e7ea866863489cad588fbaf2f31f42e5821f7a03f009b3769f05801cb1",
    executable: "pmtiles",
});

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const TOOL_ASSET: Option<ToolAsset> = Some(ToolAsset {
    name: "go-pmtiles-1.31.2_Darwin_x86_64.zip",
    sha256: "1f0dc02eee6c58312dd6c509faee1b5c32f0596568af1bf51f1b034e7a88a65b",
    executable: "pmtiles",
});

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const TOOL_ASSET: Option<ToolAsset> = Some(ToolAsset {
    name: "go-pmtiles-1.31.2_Darwin_arm64.zip",
    sha256: "40528f7f616fcbf91207cd48c8fc023d213f6d86c0cbf1f748732803d1880f3d",
    executable: "pmtiles",
});

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const TOOL_ASSET: Option<ToolAsset> = Some(ToolAsset {
    name: "go-pmtiles_1.31.2_Windows_x86_64.zip",
    sha256: "a658baa4d7e55020aef6ca17bd9ff9faa1582671266b36f58c52db0ac8e785a1",
    executable: "pmtiles.exe",
});

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
const TOOL_ASSET: Option<ToolAsset> = Some(ToolAsset {
    name: "go-pmtiles_1.31.2_Windows_arm64.zip",
    sha256: "8780a17453c63af757917a694cbbb50b943db89cc3f1b07e6fd62c1ff8e6963b",
    executable: "pmtiles.exe",
});

#[cfg(not(any(
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "aarch64"),
)))]
const TOOL_ASSET: Option<ToolAsset> = None;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    source: String,
    date: String,
    bounds: [f64; 4],
    max_zoom: u8,
    #[serde(default = "legacy_tool_version")]
    tool_version: String,
}

impl Receipt {
    fn detail_source(self) -> Result<Option<DetailSource>> {
        if self.max_zoom >= MAX_ZOOM {
            return Ok(None);
        }
        if self.max_zoom != LOCAL_MAX_ZOOM || self.bounds != BOUNDS {
            bail!("basemap provenance does not describe the installed local core");
        }
        let generation = validate_day(&self.date)?;
        let expected = format!("{MAP_ORIGIN}/{generation}.pmtiles");
        if self.source != expected {
            bail!("basemap provenance names an untrusted detail source");
        }
        Ok(Some(DetailSource {
            url: self.source,
            generation,
        }))
    }
}

#[derive(Clone, Debug)]
pub struct DetailSource {
    pub url: String,
    pub generation: String,
}

fn legacy_tool_version() -> String {
    "legacy installer".to_owned()
}

pub fn install(lair: &Lair, day: Option<&str>) -> Result<PathBuf> {
    let cancel = AtomicBool::new(false);
    install_attended(lair, day, &cancel, |_| {})
}

pub fn install_attended(
    lair: &Lair,
    day: Option<&str>,
    cancel: &AtomicBool,
    report: impl Fn(InstallProgress),
) -> Result<PathBuf> {
    if Lair::basemap_is_external() {
        bail!(
            "HRRR_BASEMAP_ARCHIVE names an externally managed archive; unset it before installing"
        );
    }
    let day = day.map_or_else(|| Ok(today_utc()), validate_day)?;
    let asset = tool_asset().context("go-pmtiles has no binary for this target")?;
    let destination = lair.basemap_path()?;
    let directory = destination
        .parent()
        .context("basemap destination has no parent")?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("create {}", directory.display()))?;
    reap_partials(directory)?;
    let arena = Arena::forge(&lair.cache_root())?;
    let package = arena.path.join(asset.name);
    let tool = arena.path.join(asset.executable);
    let url = format!("{TOOL_ORIGIN}/v{TOOL_VERSION}/{}", asset.name);

    println!("fetching verified go-pmtiles {TOOL_VERSION} for this platform");
    download(&url, &package, cancel, &report)?;
    heed(cancel)?;
    report(InstallProgress::phase(InstallPhase::CheckingTool));
    verify_sha256(&package, asset.sha256, cancel)?;
    heed(cancel)?;
    report(InstallProgress::phase(InstallPhase::UnpackingTool));
    extract_tool(&package, &tool, asset, cancel)?;
    make_executable(&tool)?;

    let source = format!("{MAP_ORIGIN}/{day}.pmtiles");
    let partial = directory.join(format!(
        "{ARCHIVE_NAME}.partial-{}-{}",
        std::process::id(),
        ARENA_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let partial_guard = EphemeralFile(partial.clone());
    let max_zoom = format!("--maxzoom={LOCAL_MAX_ZOOM}");
    println!("extracting North America through z{LOCAL_MAX_ZOOM}; this downloads about 1.1 GB");
    invoke(
        &tool,
        [
            OsStr::new("extract"),
            OsStr::new(&source),
            partial.as_os_str(),
            OsStr::new("--bbox=-135,21,-60,54"),
            OsStr::new("--minzoom=0"),
            OsStr::new(&max_zoom),
            OsStr::new("--download-threads=8"),
            OsStr::new("--overfetch=0.08"),
        ],
        "extract basemap",
        InstallPhase::ExtractingMap,
        Some(&partial),
        cancel,
        &report,
    )?;
    invoke(
        &tool,
        [OsStr::new("verify"), partial.as_os_str()],
        "verify basemap",
        InstallPhase::CheckingMap,
        Some(&partial),
        cancel,
        &report,
    )?;
    heed(cancel)?;
    replace(&partial, &destination)?;
    drop(partial_guard);
    save_toml(
        &Receipt {
            source,
            date: day,
            bounds: BOUNDS,
            max_zoom: LOCAL_MAX_ZOOM,
            tool_version: TOOL_VERSION.to_owned(),
        },
        &receipt_path(directory),
        "serialize basemap provenance",
    )?;
    println!("installed {}", destination.display());
    Ok(destination)
}

pub fn was_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Cancelled>().is_some()
}

pub fn detail_source(lair: &Lair) -> Result<Option<DetailSource>> {
    if Lair::basemap_is_external() {
        return Ok(None);
    }
    let archive = lair.basemap_path()?;
    let directory = archive.parent().context("basemap archive has no parent")?;
    let receipt = match std::fs::read_to_string(receipt_path(directory)) {
        Ok(receipt) => receipt,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("read basemap provenance"),
    };
    let receipt: Receipt = toml::from_str(&receipt).context("parse basemap provenance")?;
    receipt.detail_source()
}

pub fn status(lair: &Lair) -> Result<()> {
    let archive = lair.basemap_path()?;
    let metadata = std::fs::metadata(&archive).with_context(|| {
        format!(
            "no basemap archive at {}; run `hrrr basemap install`",
            archive.display()
        )
    })?;
    if Lair::basemap_is_external() {
        println!(
            "{}\n{} bytes · externally managed",
            archive.display(),
            metadata.len()
        );
        return Ok(());
    }
    let receipt = std::fs::read_to_string(receipt_path(
        archive.parent().context("basemap archive has no parent")?,
    ))
    .context("read basemap provenance")?;
    let receipt: Receipt = toml::from_str(&receipt).context("parse basemap provenance")?;
    println!(
        "{}\n{} bytes · source {} · z{} · go-pmtiles {}",
        archive.display(),
        metadata.len(),
        receipt.date,
        receipt.max_zoom,
        receipt.tool_version
    );
    Ok(())
}

pub fn remove(lair: &Lair) -> Result<()> {
    if Lair::basemap_is_external() {
        bail!("HRRR_BASEMAP_ARCHIVE names an externally managed archive; HRRR will not remove it");
    }
    let archive = lair.basemap_path()?;
    let directory = archive.parent().context("basemap archive has no parent")?;
    let installed = archive.is_file();
    for path in [
        archive.clone(),
        receipt_path(directory),
        receipt_path(directory).with_extension("toml.bak"),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("remove {}", path.display())),
        }
    }
    lair.basemap_cache().clear()?;
    match std::fs::remove_dir(directory) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) => {}
        Err(error) => {
            return Err(error).with_context(|| format!("remove {}", directory.display()));
        }
    }
    println!(
        "{}",
        if installed {
            format!("removed {} and its cached detail", archive.display())
        } else {
            "no basemap is installed; cached detail cleared".to_owned()
        }
    );
    Ok(())
}

fn today_utc() -> String {
    Timestamp::now()
        .to_zoned(TimeZone::UTC)
        .strftime("%Y%m%d")
        .to_string()
}

fn validate_day(day: &str) -> Result<String> {
    let _date = Date::strptime("%Y%m%d", day).context("basemap date must be YYYYMMDD")?;
    Ok(day.to_owned())
}

fn tool_asset() -> Option<ToolAsset> {
    TOOL_ASSET
}

fn download(
    url: &str,
    path: &Path,
    cancel: &AtomicBool,
    report: &impl Fn(InstallProgress),
) -> Result<()> {
    let response = ureq::Agent::new_with_defaults()
        .get(url)
        .call()
        .with_context(|| format!("fetch {url}"))?;
    let total = response.body().content_length();
    let mut reader = response.into_body().into_reader();
    let mut file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut bytes = 0_u64;
    loop {
        heed(cancel)?;
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("download {url}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .with_context(|| format!("write {}", path.display()))?;
        bytes += read as u64;
        report(InstallProgress::bytes(
            InstallPhase::FetchingTool,
            bytes,
            total,
        ));
    }
    file.sync_all()
        .with_context(|| format!("sync {}", path.display()))
}

fn verify_sha256(path: &Path, expected: &str, cancel: &AtomicBool) -> Result<()> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        heed(cancel)?;
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("hash {}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = format!("{:x}", digest.finalize());
    if actual != expected {
        bail!(
            "SHA-256 mismatch for {}: expected {expected}, got {actual}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn extract_tool(package: &Path, tool: &Path, asset: ToolAsset, cancel: &AtomicBool) -> Result<()> {
    let file = File::open(package).with_context(|| format!("open {}", package.display()))?;
    for entry in tar::Archive::new(GzDecoder::new(file)).entries()? {
        heed(cancel)?;
        let mut entry = entry?;
        if entry.path()?.file_name() == Some(OsStr::new(asset.executable)) {
            let mut output = File::create(tool)?;
            let _bytes = std::io::copy(&mut entry, &mut output)?;
            output.sync_all()?;
            return Ok(());
        }
    }
    bail!("{} contains no {}", package.display(), asset.executable)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn extract_tool(package: &Path, tool: &Path, asset: ToolAsset, cancel: &AtomicBool) -> Result<()> {
    let file = File::open(package).with_context(|| format!("open {}", package.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("open go-pmtiles zip")?;
    for slot in 0..zip.len() {
        heed(cancel)?;
        let mut entry = zip.by_index(slot)?;
        if Path::new(entry.name()).file_name() == Some(OsStr::new(asset.executable)) {
            let mut output = File::create(tool)?;
            let _bytes = std::io::copy(&mut entry, &mut output)?;
            output.sync_all()?;
            return Ok(());
        }
    }
    bail!("{} contains no {}", package.display(), asset.executable)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn extract_tool(
    _package: &Path,
    _tool: &Path,
    _asset: ToolAsset,
    _cancel: &AtomicBool,
) -> Result<()> {
    bail!("basemap installation is unsupported on this platform")
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn invoke<const N: usize>(
    tool: &Path,
    arguments: [&OsStr; N],
    operation: &'static str,
    phase: InstallPhase,
    artifact: Option<&Path>,
    cancel: &AtomicBool,
    report: &impl Fn(InstallProgress),
) -> Result<()> {
    report(InstallProgress::phase(phase));
    let mut command = Command::new(tool);
    let _command = command.args(arguments);
    hide_child_console(&mut command);
    let mut child = command
        .spawn()
        .with_context(|| format!("{operation} with {}", tool.display()))?;
    loop {
        if cancel.load(Ordering::Acquire) {
            let _killed = child.kill();
            let _reaped = child.wait();
            return Err(Cancelled.into());
        }
        if let Some(status) = child.try_wait().context("poll go-pmtiles")? {
            if !status.success() {
                bail!("{operation} failed with {status}");
            }
            return Ok(());
        }
        let bytes = artifact
            .and_then(|path| std::fs::metadata(path).ok())
            .map_or(0, |metadata| metadata.len());
        report(InstallProgress::bytes(phase, bytes, None));
        std::thread::sleep(CHILD_POLL);
    }
}

#[cfg(windows)]
fn hide_child_console(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _command = command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_child_console(_command: &mut Command) {}

fn heed(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Cancelled.into())
    } else {
        Ok(())
    }
}

fn reap_partials(directory: &Path) -> Result<()> {
    let prefix = format!("{ARCHIVE_NAME}.partial-");
    for entry in
        std::fs::read_dir(directory).with_context(|| format!("inspect {}", directory.display()))?
    {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) && entry.file_type()?.is_file()
        {
            std::fs::remove_file(entry.path())
                .with_context(|| format!("remove abandoned {}", entry.path().display()))?;
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace(source: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(source, destination)
        .with_context(|| format!("install {} as {}", source.display(), destination.display()))
}

#[cfg(windows)]
fn replace(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        std::fs::remove_file(destination)
            .with_context(|| format!("remove {}", destination.display()))?;
    }
    std::fs::rename(source, destination)
        .with_context(|| format!("install {} as {}", source.display(), destination.display()))
}

fn receipt_path(directory: &Path) -> PathBuf {
    directory.join("source.toml")
}

struct Arena {
    path: PathBuf,
}

impl Arena {
    fn forge(cache: &Path) -> Result<Self> {
        let path = cache.join(format!(
            "basemap-install-{}-{}",
            std::process::id(),
            ARENA_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(Self { path })
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.path);
    }
}

struct EphemeralFile(PathBuf);

impl Drop for EphemeralFile {
    fn drop(&mut self) {
        let _removed = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_asset_is_pinned_and_digest_shaped() -> Result<()> {
        let asset = tool_asset().context("release host has no pinned go-pmtiles artifact")?;
        assert_eq!(asset.sha256.len(), 64);
        assert!(asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(asset.name.contains(TOOL_VERSION));
        Ok(())
    }

    #[test]
    fn dates_are_civil_dates_not_digit_soup() -> Result<()> {
        assert_eq!(validate_day("20260728")?.as_str(), "20260728");
        assert!(validate_day("20260230").is_err());
        assert!(validate_day("../../28").is_err());
        Ok(())
    }

    #[test]
    fn detail_authority_is_derived_only_from_exact_provenance() -> Result<()> {
        let receipt = |date: &str, source: &str, bounds, max_zoom| Receipt {
            source: source.to_owned(),
            date: date.to_owned(),
            bounds,
            max_zoom,
            tool_version: TOOL_VERSION.to_owned(),
        };
        let source = receipt(
            "20260809",
            "https://build.protomaps.com/20260809.pmtiles",
            BOUNDS,
            LOCAL_MAX_ZOOM,
        )
        .detail_source()?
        .context("lawful core omitted detail authority")?;
        assert_eq!(source.generation, "20260809");
        assert!(
            receipt(
                "../../09",
                "https://build.protomaps.com/../../09.pmtiles",
                BOUNDS,
                LOCAL_MAX_ZOOM,
            )
            .detail_source()
            .is_err()
        );
        assert!(
            receipt(
                "20260809",
                "https://example.com/20260809.pmtiles",
                BOUNDS,
                LOCAL_MAX_ZOOM,
            )
            .detail_source()
            .is_err()
        );
        assert!(
            receipt(
                "20260809",
                "https://build.protomaps.com/20260809.pmtiles",
                [-180.0, -85.0, 180.0, 85.0],
                LOCAL_MAX_ZOOM,
            )
            .detail_source()
            .is_err()
        );
        assert!(
            receipt(
                "20260809",
                "https://build.protomaps.com/20260809.pmtiles",
                BOUNDS,
                LOCAL_MAX_ZOOM - 1,
            )
            .detail_source()
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn cancellation_is_typed() -> Result<()> {
        let cancel = AtomicBool::new(true);
        let Err(error) = heed(&cancel) else {
            bail!("set cancellation did not abort");
        };
        assert!(was_cancelled(&error));
        Ok(())
    }

    #[test]
    fn abandoned_partials_are_reaped_without_touching_neighbors() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "hrrr-partial-reaping-{}-{}",
            std::process::id(),
            ARENA_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root)?;
        let partial = root.join(format!("{ARCHIVE_NAME}.partial-dead"));
        let neighbor = root.join("keep.pmtiles");
        let _partial = File::create(&partial)?;
        let _neighbor = File::create(&neighbor)?;
        reap_partials(&root)?;
        assert!(!partial.exists());
        assert!(neighbor.exists());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
