use anyhow::{Context as _, Result, bail};
use atomic_write_file::AtomicWriteFile;
use crossbeam_channel::{Receiver, Sender, bounded};
use egui::Context;
use std::{
    fs::File,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const MEBIBYTE: u64 = 1_048_576;
const REAP_INTERVAL: Duration = Duration::from_hours(6);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheClass {
    Field,
}

impl CacheClass {
    const ALL: [Self; 1] = [Self::Field];

    const fn directory(self) -> &'static str {
        match self {
            Self::Field => "fields",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CachePolicy {
    stale_after: Duration,
    byte_ceiling: u64,
}

const STANDARD_POLICY: CachePolicy = CachePolicy {
    stale_after: Duration::from_hours(7 * 24),
    byte_ceiling: 512 * MEBIBYTE,
};

#[derive(Clone, Debug)]
pub struct CacheManager {
    root: PathBuf,
    policy: CachePolicy,
    gate: Arc<RwLock<()>>,
}

impl CacheManager {
    pub fn standard(root: PathBuf) -> Self {
        Self {
            root,
            policy: STANDARD_POLICY,
            gate: Arc::default(),
        }
    }

    pub fn store(&self, class: CacheClass) -> CacheStore {
        CacheStore {
            root: self.root.join(class.directory()),
            gate: self.gate.clone(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn purge(&self) -> Result<PurgeTally> {
        let _guard = write_gate(&self.gate);
        let mut tally = PurgeTally::default();
        for class in CacheClass::ALL {
            tally.absorb(purge_domain(
                &self.root.join(class.directory()),
                self.policy,
            )?);
        }
        Ok(tally)
    }
}

#[derive(Clone, Debug)]
pub struct CacheStore {
    root: PathBuf,
    gate: Arc<RwLock<()>>,
}

impl CacheStore {
    pub fn read(&self, blade: &Path) -> std::io::Result<Vec<u8>> {
        let _guard = read_gate(&self.gate);
        let path = self.root.join(blade);
        let mut file = File::open(&path)?;
        let mut bytes = Vec::new();
        let _read = file.read_to_end(&mut bytes)?;
        let _renewed = file.set_modified(SystemTime::now());
        Ok(bytes)
    }

    pub fn remove(&self, blade: &Path) -> std::io::Result<()> {
        let _guard = read_gate(&self.gate);
        std::fs::remove_file(self.root.join(blade))
    }

    pub fn write(&self, blade: &Path, bytes: &[u8]) -> Result<()> {
        let _guard = read_gate(&self.gate);
        let path = self.root.join(blade);
        let Some(parent) = path.parent() else {
            bail!("cache blade {} has no parent", path.display());
        };
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create cache chamber {}", parent.display()))?;
        let mut file = AtomicWriteFile::open(&path)
            .with_context(|| format!("stage cache blade {}", path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write cache blade {}", path.display()))?;
        file.commit()
            .with_context(|| format!("commit cache blade {}", path.display()))
    }

    pub fn resolve(
        &self,
        blade: &Path,
        valid: impl Fn(&[u8]) -> bool,
        fetch: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        match self.read(blade) {
            Ok(bytes) if valid(&bytes) => return Ok(bytes),
            Ok(_) => self
                .remove(blade)
                .with_context(|| format!("discard corrupt cache blade {}", blade.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", blade.display()));
            }
        }
        let bytes = fetch()?;
        if !valid(&bytes) {
            bail!("fetched invalid cache blade {}", blade.display());
        }
        self.write(blade, &bytes)?;
        Ok(bytes)
    }
}

fn read_gate(gate: &RwLock<()>) -> RwLockReadGuard<'_, ()> {
    match gate.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn write_gate(gate: &RwLock<()>) -> RwLockWriteGuard<'_, ()> {
    match gate.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PurgeTally {
    pub files: usize,
    pub bytes: u64,
}

impl PurgeTally {
    fn absorb(&mut self, other: Self) {
        self.files = self.files.saturating_add(other.files);
        self.bytes = self.bytes.saturating_add(other.bytes);
    }
}

pub struct Custodian {
    shutdown: Sender<()>,
    pub faults: Receiver<String>,
    _thread: thread::JoinHandle<()>,
}

impl Custodian {
    pub fn spawn(ctx: Context, manager: CacheManager) -> Result<Self> {
        let (shutdown, orders) = bounded(1);
        let (fault_tx, faults) = bounded(1);
        let thread = thread::Builder::new()
            .name("cache-custodian".to_owned())
            .spawn(move || tend_cache(ctx, manager, orders, fault_tx))
            .context("spawn cache custodian")?;
        Ok(Self {
            shutdown,
            faults,
            _thread: thread,
        })
    }
}

impl Drop for Custodian {
    fn drop(&mut self) {
        let _sent = self.shutdown.try_send(());
    }
}

fn tend_cache(ctx: Context, manager: CacheManager, orders: Receiver<()>, faults: Sender<String>) {
    loop {
        if let Err(err) = manager.purge() {
            let _sent = faults.try_send(format!("cache purge failed: {err:#}"));
            ctx.request_repaint();
        }
        match orders.recv_timeout(REAP_INTERVAL) {
            Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[derive(Debug)]
struct Blade {
    path: PathBuf,
    modified: SystemTime,
    bytes: u64,
    regular: bool,
}

fn purge_domain(root: &Path, policy: CachePolicy) -> Result<PurgeTally> {
    let mut blades = Vec::new();
    let mut directories = Vec::new();
    inventory(root, &mut blades, &mut directories)?;
    let now = SystemTime::now();
    let mut tally = PurgeTally::default();
    let mut retained = Vec::with_capacity(blades.len());
    let mut retained_bytes = 0_u64;

    for blade in blades {
        let age = now.duration_since(blade.modified).unwrap_or_default();
        if !blade.regular || age > policy.stale_after {
            reap_blade(&blade, &mut tally)?;
        } else {
            retained_bytes = retained_bytes.saturating_add(blade.bytes);
            retained.push(blade);
        }
    }

    retained.sort_unstable_by_key(|blade| blade.modified);
    for blade in retained {
        if retained_bytes <= policy.byte_ceiling {
            break;
        }
        reap_blade(&blade, &mut tally)?;
        retained_bytes = retained_bytes.saturating_sub(blade.bytes);
    }

    for directory in directories {
        match std::fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("prune cache chamber {}", directory.display()));
            }
        }
    }
    Ok(tally)
}

fn inventory(root: &Path, blades: &mut Vec<Blade>, directories: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect cache domain {}", root.display()));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("read cache domain {}", root.display()))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("inspect cache blade {}", path.display()))?;
        if metadata.is_dir() {
            inventory(&path, blades, directories)?;
            directories.push(path);
        } else {
            blades.push(Blade {
                path,
                modified: metadata.modified().unwrap_or(UNIX_EPOCH),
                bytes: metadata.len(),
                regular: metadata.is_file(),
            });
        }
    }
    Ok(())
}

fn reap_blade(blade: &Blade, tally: &mut PurgeTally) -> Result<()> {
    match std::fs::remove_file(&blade.path) {
        Ok(()) => {
            tally.files = tally.files.saturating_add(1);
            tally.bytes = tally.bytes.saturating_add(blade.bytes);
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("reap cache blade {}", blade.path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::sync::Barrier;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn forge() -> Result<Self> {
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let path =
                std::env::temp_dir().join(format!("hrrr-cache-{}-{nonce}", std::process::id()));
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
    fn age_law_reaps_blades_and_empty_chambers() -> Result<()> {
        let root = TestRoot::forge()?;
        let manager = CacheManager {
            root: root.0.clone(),
            policy: CachePolicy {
                stale_after: Duration::ZERO,
                byte_ceiling: u64::MAX,
            },
            gate: Arc::default(),
        };
        let store = manager.store(CacheClass::Field);
        let blade = Path::new("run/f00/qpf.grib2");
        store.write(blade, b"GRIB7777")?;
        File::open(store.root.join(blade))?.set_modified(UNIX_EPOCH)?;
        let tally = manager.purge()?;
        assert_eq!(tally, PurgeTally { files: 1, bytes: 8 });
        assert!(!manager.store(CacheClass::Field).root.join("run").exists());
        Ok(())
    }

    #[test]
    fn byte_law_cuts_an_oversized_domain() -> Result<()> {
        let root = TestRoot::forge()?;
        let manager = CacheManager {
            root: root.0.clone(),
            policy: CachePolicy {
                stale_after: Duration::MAX,
                byte_ceiling: 3,
            },
            gate: Arc::default(),
        };
        manager
            .store(CacheClass::Field)
            .write(Path::new("oversized"), b"four")?;
        assert_eq!(manager.purge()?.bytes, 4);
        Ok(())
    }

    #[test]
    fn reads_renew_a_blades_lifetime() -> Result<()> {
        let root = TestRoot::forge()?;
        let manager = CacheManager {
            root: root.0.clone(),
            policy: CachePolicy {
                stale_after: Duration::from_hours(1),
                byte_ceiling: u64::MAX,
            },
            gate: Arc::default(),
        };
        let store = manager.store(CacheClass::Field);
        let blade = Path::new("still-warm");
        store.write(blade, b"warm")?;
        File::open(store.root.join(blade))?.set_modified(UNIX_EPOCH)?;
        assert_eq!(store.read(blade)?, b"warm");
        assert_eq!(manager.purge()?, PurgeTally::default());
        assert!(store.root.join(blade).exists());
        Ok(())
    }

    #[test]
    fn concurrent_writers_never_share_a_temporary_blade() -> Result<()> {
        let root = TestRoot::forge()?;
        let manager = CacheManager::standard(root.0.clone());
        let blade = Path::new("same/index");
        let barrier = Arc::new(Barrier::new(2));
        let forge = |store: CacheStore, barrier: Arc<Barrier>, byte| {
            thread::spawn(move || {
                let payload = vec![byte; MEBIBYTE as usize];
                let _released = barrier.wait();
                store.write(blade, &payload)
            })
        };
        let left = forge(manager.store(CacheClass::Field), barrier.clone(), b'L');
        let right = forge(manager.store(CacheClass::Field), barrier, b'R');
        left.join()
            .map_err(|_| anyhow!("left cache writer panicked"))??;
        right
            .join()
            .map_err(|_| anyhow!("right cache writer panicked"))??;

        let payload = manager.store(CacheClass::Field).read(blade)?;
        assert_eq!(payload.len(), MEBIBYTE as usize);
        assert!(
            payload.iter().all(|byte| *byte == b'L') || payload.iter().all(|byte| *byte == b'R')
        );
        Ok(())
    }
}
