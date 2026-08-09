use crate::{persist::save_toml, xdg::Lair};
use anyhow::{Context as _, Result, bail};
use flate2::read::GzDecoder;
use jiff::{Timestamp, civil::Date, tz::TimeZone};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    ffi::OsStr,
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

pub const ARCHIVE_NAME: &str = "basemap.pmtiles";
pub const MAX_ZOOM: u8 = 12;
pub const BOUNDS: [f64; 4] = [-135.0, 21.0, -60.0, 54.0];
const TOOL_VERSION: &str = "1.31.2";
const TOOL_ORIGIN: &str = "https://github.com/protomaps/go-pmtiles/releases/download";
const MAP_ORIGIN: &str = "https://build.protomaps.com";
static ARENA_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
enum Package {
    TarGz,
    Zip,
}

#[derive(Clone, Copy)]
struct ToolAsset {
    name: &'static str,
    sha256: &'static str,
    package: Package,
    executable: &'static str,
}

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

fn legacy_tool_version() -> String {
    "legacy installer".to_owned()
}

pub fn install(lair: &Lair, day: Option<&str>) -> Result<PathBuf> {
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
    let arena = Arena::forge(&lair.cache_root())?;
    let package = arena.path.join(asset.name);
    let tool = arena.path.join(asset.executable);
    let url = format!("{TOOL_ORIGIN}/v{TOOL_VERSION}/{}", asset.name);

    println!("fetching verified go-pmtiles {TOOL_VERSION} for this platform");
    download(&url, &package)?;
    verify_sha256(&package, asset.sha256)?;
    extract_tool(&package, &tool, asset)?;
    make_executable(&tool)?;

    let source = format!("{MAP_ORIGIN}/{day}.pmtiles");
    let partial = directory.join(format!(
        "{ARCHIVE_NAME}.partial-{}-{}",
        std::process::id(),
        ARENA_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let partial_guard = EphemeralFile(partial.clone());
    println!("extracting North America through z{MAX_ZOOM}; this downloads several GiB");
    invoke(
        &tool,
        [
            OsStr::new("extract"),
            OsStr::new(&source),
            partial.as_os_str(),
            OsStr::new("--bbox=-135,21,-60,54"),
            OsStr::new("--minzoom=0"),
            OsStr::new("--maxzoom=12"),
            OsStr::new("--download-threads=8"),
            OsStr::new("--overfetch=0.08"),
        ],
        "extract basemap",
    )?;
    invoke(
        &tool,
        [OsStr::new("verify"), partial.as_os_str()],
        "verify basemap",
    )?;
    replace(&partial, &destination)?;
    drop(partial_guard);
    save_toml(
        &Receipt {
            source,
            date: day,
            bounds: BOUNDS,
            max_zoom: MAX_ZOOM,
            tool_version: TOOL_VERSION.to_owned(),
        },
        &receipt_path(directory),
        "serialize basemap provenance",
    )?;
    println!("installed {}", destination.display());
    Ok(destination)
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
    match std::fs::remove_file(&archive) {
        Ok(()) => {
            for path in [
                receipt_path(directory),
                receipt_path(directory).with_extension("toml.bak"),
            ] {
                match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| format!("remove {}", path.display()));
                    }
                }
            }
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
            println!("removed {}", archive.display());
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("no basemap is installed");
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("remove {}", directory.display())),
    }
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
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some(ToolAsset {
            name: "go-pmtiles_1.31.2_Linux_x86_64.tar.gz",
            sha256: "3ed7dbf4ec2e6dfe5e25b6f70d1ffc932729f93c86db353bf514dd71010a312f",
            package: Package::TarGz,
            executable: "pmtiles",
        }),
        ("linux", "aarch64") => Some(ToolAsset {
            name: "go-pmtiles_1.31.2_Linux_arm64.tar.gz",
            sha256: "f8bd47e7ea866863489cad588fbaf2f31f42e5821f7a03f009b3769f05801cb1",
            package: Package::TarGz,
            executable: "pmtiles",
        }),
        ("macos", "x86_64") => Some(ToolAsset {
            name: "go-pmtiles-1.31.2_Darwin_x86_64.zip",
            sha256: "1f0dc02eee6c58312dd6c509faee1b5c32f0596568af1bf51f1b034e7a88a65b",
            package: Package::Zip,
            executable: "pmtiles",
        }),
        ("macos", "aarch64") => Some(ToolAsset {
            name: "go-pmtiles-1.31.2_Darwin_arm64.zip",
            sha256: "40528f7f616fcbf91207cd48c8fc023d213f6d86c0cbf1f748732803d1880f3d",
            package: Package::Zip,
            executable: "pmtiles",
        }),
        ("windows", "x86_64") => Some(ToolAsset {
            name: "go-pmtiles_1.31.2_Windows_x86_64.zip",
            sha256: "a658baa4d7e55020aef6ca17bd9ff9faa1582671266b36f58c52db0ac8e785a1",
            package: Package::Zip,
            executable: "pmtiles.exe",
        }),
        ("windows", "aarch64") => Some(ToolAsset {
            name: "go-pmtiles_1.31.2_Windows_arm64.zip",
            sha256: "8780a17453c63af757917a694cbbb50b943db89cc3f1b07e6fd62c1ff8e6963b",
            package: Package::Zip,
            executable: "pmtiles.exe",
        }),
        _ => None,
    }
}

fn download(url: &str, path: &Path) -> Result<()> {
    let response = ureq::Agent::new_with_defaults()
        .get(url)
        .call()
        .with_context(|| format!("fetch {url}"))?;
    let mut reader = response.into_body().into_reader();
    let mut file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let _bytes =
        std::io::copy(&mut reader, &mut file).with_context(|| format!("download {url}"))?;
    file.sync_all()
        .with_context(|| format!("sync {}", path.display()))
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
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

fn extract_tool(package: &Path, tool: &Path, asset: ToolAsset) -> Result<()> {
    match asset.package {
        Package::TarGz => {
            let file =
                File::open(package).with_context(|| format!("open {}", package.display()))?;
            for entry in tar::Archive::new(GzDecoder::new(file)).entries()? {
                let mut entry = entry?;
                if entry.path()?.file_name() == Some(OsStr::new(asset.executable)) {
                    let mut output = File::create(tool)?;
                    let _bytes = std::io::copy(&mut entry, &mut output)?;
                    output.sync_all()?;
                    return Ok(());
                }
            }
        }
        Package::Zip => {
            let file =
                File::open(package).with_context(|| format!("open {}", package.display()))?;
            let mut zip = zip::ZipArchive::new(file).context("open go-pmtiles zip")?;
            for slot in 0..zip.len() {
                let mut entry = zip.by_index(slot)?;
                if Path::new(entry.name()).file_name() == Some(OsStr::new(asset.executable)) {
                    let mut output = File::create(tool)?;
                    let _bytes = std::io::copy(&mut entry, &mut output)?;
                    output.sync_all()?;
                    return Ok(());
                }
            }
        }
    }
    bail!("{} contains no {}", package.display(), asset.executable)
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
) -> Result<()> {
    let status = Command::new(tool)
        .args(arguments)
        .status()
        .with_context(|| format!("{operation} with {}", tool.display()))?;
    if !status.success() {
        bail!("{operation} failed with {status}");
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
}
