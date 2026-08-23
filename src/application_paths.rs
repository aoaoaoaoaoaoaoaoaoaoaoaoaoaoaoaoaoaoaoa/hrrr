use crate::cache::{CacheClass, CacheManager, CacheStore};
use anyhow::{Context as _, Result, bail};
use directories::ProjectDirs;
use std::{
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct ApplicationPaths {
    pub config: PathBuf,
    pub state: PathBuf,
    pub data: PathBuf,
    local_data: PathBuf,
    cache: CacheManager,
}

#[derive(Debug)]
pub struct InstanceGuard {
    _file: File,
}

impl ApplicationPaths {
    pub fn claim() -> Result<Self> {
        let Some(dirs) = ProjectDirs::from("moe", "swarm", "hrrr") else {
            bail!("could not resolve platform project directories");
        };
        let state = dirs
            .state_dir()
            .map_or_else(|| dirs.data_local_dir().join("state"), Path::to_path_buf);
        let cache = CacheManager::standard(dirs.cache_dir().to_path_buf());
        Ok(Self {
            config: dirs.config_dir().to_path_buf(),
            state,
            data: dirs.data_dir().to_path_buf(),
            local_data: dirs.data_local_dir().to_path_buf(),
            cache,
        })
    }

    pub fn session_state_path(&self) -> PathBuf {
        // The legacy filename is durable XDG state ABI; only the Rust noun was
        // rectified.
        self.state.join("slate.toml")
    }

    pub fn config_path(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    pub fn views_path(&self) -> PathBuf {
        self.data.join("views.toml")
    }

    pub fn field_cache(&self) -> CacheStore {
        self.cache.store(CacheClass::Field)
    }

    pub fn basemap_cache(&self) -> CacheStore {
        self.cache.store(CacheClass::Basemap)
    }

    pub fn cache_root(&self) -> PathBuf {
        self.cache.root().to_path_buf()
    }

    pub fn basemap_path(&self) -> Result<PathBuf> {
        let path = std::env::var_os("HRRR_BASEMAP_ARCHIVE").map_or_else(
            || managed_basemap(&self.data, &self.local_data),
            PathBuf::from,
        );
        if path.is_absolute() {
            Ok(path)
        } else {
            bail!("HRRR_BASEMAP_ARCHIVE must be an absolute path")
        }
    }

    pub fn basemap_is_external() -> bool {
        std::env::var_os("HRRR_BASEMAP_ARCHIVE").is_some()
    }

    pub fn cache_manager(&self) -> CacheManager {
        self.cache.clone()
    }

    pub fn lock_instance(&self) -> Result<InstanceGuard> {
        let root = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map_or_else(|| self.state.clone(), |path| path.join("hrrr"));
        InstanceGuard::claim(&root)
    }
}

fn managed_basemap(roaming: &Path, local: &Path) -> PathBuf {
    let local = local
        .join("basemap")
        .join(crate::basemap_artifact::ARCHIVE_NAME);
    let legacy = roaming
        .join("basemap")
        .join(crate::basemap_artifact::ARCHIVE_NAME);
    if local.is_file() || !legacy.is_file() {
        local
    } else {
        legacy
    }
}

impl InstanceGuard {
    fn claim(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
        let path = root.join("instance.lock");
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        file.try_lock()
            .context("another HRRR instance already owns the application state")?;
        Ok(InstanceGuard { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn one_state_domain_admits_one_writer() -> Result<()> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!("hrrr-lock-{}-{nonce}", std::process::id()));
        let first = InstanceGuard::claim(&root)?;
        assert!(InstanceGuard::claim(&root).is_err());
        drop(first);
        let _successor = InstanceGuard::claim(&root)?;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn machine_local_basemaps_supersede_roaming_legacy_archives() -> Result<()> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root =
            std::env::temp_dir().join(format!("hrrr-basemap-root-{}-{nonce}", std::process::id()));
        let roaming = root.join("roaming");
        let local = root.join("local");
        let local_archive = local
            .join("basemap")
            .join(crate::basemap_artifact::ARCHIVE_NAME);
        let legacy_archive = roaming
            .join("basemap")
            .join(crate::basemap_artifact::ARCHIVE_NAME);

        assert_eq!(managed_basemap(&roaming, &local), local_archive);
        std::fs::create_dir_all(legacy_archive.parent().context("legacy parent")?)?;
        let _legacy = File::create(&legacy_archive)?;
        assert_eq!(managed_basemap(&roaming, &local), legacy_archive);
        std::fs::create_dir_all(local_archive.parent().context("local parent")?)?;
        let _local = File::create(&local_archive)?;
        assert_eq!(managed_basemap(&roaming, &local), local_archive);

        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
