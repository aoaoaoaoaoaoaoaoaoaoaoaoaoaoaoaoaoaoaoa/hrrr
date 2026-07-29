use anyhow::{Context as _, Result, anyhow};
use atomic_write_file::AtomicWriteFile;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static QUARANTINE_NONCE: AtomicU64 = AtomicU64::new(0);

pub fn load_toml<T: DeserializeOwned>(path: &Path, what: &'static str) -> Result<Option<T>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    match toml::from_str(&text) {
        Ok(value) => Ok(Some(value)),
        Err(primary_error) => recover_backup(path, what, primary_error),
    }
}

pub fn save_toml(value: &impl Serialize, path: &Path, what: &'static str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(value).context(what)?;
    match std::fs::read(path) {
        Ok(previous) => atomic_write(&backup_path(path), &previous)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("back up {}", path.display())),
    }
    atomic_write(path, text.as_bytes())
}

fn recover_backup<T: DeserializeOwned>(
    path: &Path,
    what: &'static str,
    primary_error: toml::de::Error,
) -> Result<Option<T>> {
    let backup = backup_path(path);
    let text = std::fs::read_to_string(&backup).with_context(|| {
        format!(
            "parse {} as {what}: {primary_error}; no readable backup at {}",
            path.display(),
            backup.display()
        )
    })?;
    let value = toml::from_str(&text).map_err(|backup_error| {
        anyhow!(
            "parse {} as {what}: {primary_error}; backup {} is also invalid: {backup_error}",
            path.display(),
            backup.display()
        )
    })?;
    let quarantine = quarantine_path(path);
    std::fs::rename(path, &quarantine).with_context(|| {
        format!(
            "preserve corrupt {} as {}",
            path.display(),
            quarantine.display()
        )
    })?;
    Ok(Some(value))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file =
        AtomicWriteFile::open(path).with_context(|| format!("stage {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("write {}", path.display()))?;
    file.commit()
        .with_context(|| format!("commit {}", path.display()))
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension(path.extension().map_or_else(
        || "bak".into(),
        |extension| format!("{}.bak", extension.to_string_lossy()),
    ))
}

fn quarantine_path(path: &Path) -> PathBuf {
    let nonce = QUARANTINE_NONCE.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!(
        "{}.corrupt-{}-{nonce}",
        path.extension().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use serde::{Deserialize, Serialize};
    use std::{
        sync::{Arc, Barrier},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct Testament {
        generation: u8,
        payload: String,
    }

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn forge() -> Result<Self> {
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let path =
                std::env::temp_dir().join(format!("hrrr-persist-{}-{nonce}", std::process::id()));
            std::fs::create_dir(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _removed = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn corrupt_primary_retreats_to_the_last_complete_generation() -> Result<()> {
        let root = TestRoot::forge()?;
        let path = root.0.join("slate.toml");
        let first = Testament {
            generation: 1,
            payload: "first".to_owned(),
        };
        let second = Testament {
            generation: 2,
            payload: "second".to_owned(),
        };
        save_toml(&first, &path, "serialize testament")?;
        save_toml(&second, &path, "serialize testament")?;
        std::fs::write(&path, "generation = [")?;

        assert_eq!(load_toml(&path, "testament")?, Some(first));
        assert!(!path.exists());
        assert!(root.0.read_dir()?.any(|entry| {
            entry.is_ok_and(|entry| entry.file_name().to_string_lossy().contains(".corrupt-"))
        }));
        Ok(())
    }

    #[test]
    fn rival_writers_can_only_commit_whole_documents() -> Result<()> {
        let root = TestRoot::forge()?;
        let path = Arc::new(root.0.join("views.toml"));
        let barrier = Arc::new(Barrier::new(2));
        let writer = |generation, byte| {
            let path = path.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let value = Testament {
                    generation,
                    payload: char::from(byte).to_string().repeat(256 * 1024),
                };
                let _released = barrier.wait();
                save_toml(&value, &path, "serialize testament")
            })
        };
        let left = writer(1, b'L');
        let right = writer(2, b'R');
        left.join()
            .map_err(|_| anyhow!("left persistence writer panicked"))??;
        right
            .join()
            .map_err(|_| anyhow!("right persistence writer panicked"))??;

        let value = load_toml::<Testament>(&path, "testament")?.context("committed document")?;
        let expected = if value.generation == 1 { 'L' } else { 'R' };
        assert_eq!(value.payload.len(), 256 * 1024);
        assert!(value.payload.chars().all(|character| character == expected));
        Ok(())
    }
}
