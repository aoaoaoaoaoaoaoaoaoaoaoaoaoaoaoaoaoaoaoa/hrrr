use crate::cache::{CacheClass, CacheManager, CacheStore};
use anyhow::{Context as _, Result, bail};
use directories::ProjectDirs;
use fs4::fs_std::FileExt as _;
use std::{
    fs::File,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Lair {
    pub config: PathBuf,
    pub state: PathBuf,
    pub data: PathBuf,
    cache: CacheManager,
}

#[derive(Debug)]
pub struct InstanceGuard {
    _file: File,
}

impl Lair {
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
            cache,
        })
    }

    pub fn slate_path(&self) -> PathBuf {
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

    pub fn cache_root(&self) -> PathBuf {
        self.cache.root().to_path_buf()
    }

    pub fn basemap_path(&self) -> Result<PathBuf> {
        let path = std::env::var_os("HRRR_BASEMAP_ARCHIVE").map_or_else(
            || {
                self.data
                    .join("basemap")
                    .join(crate::basemap_artifact::ARCHIVE_NAME)
            },
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
        file.try_lock_exclusive()
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
    fn platform_project_roots_are_absolute() -> Result<()> {
        let lair = Lair::claim()?;
        for path in [
            lair.config.clone(),
            lair.state.clone(),
            lair.data.clone(),
            lair.cache_root(),
        ] {
            assert!(
                path.is_absolute(),
                "relative product root: {}",
                path.display()
            );
        }
        Ok(())
    }
}
