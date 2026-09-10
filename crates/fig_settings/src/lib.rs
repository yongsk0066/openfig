pub mod error;
pub mod history;
pub mod keybindings;
pub mod settings;
pub mod sqlite;
pub mod state;

use std::fs::{
    self,
    File,
};
use std::io::{
    Read,
    Write,
};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

pub use error::{
    Error,
    Result,
};
use fd_lock::RwLock as FileRwLock;
use fig_util::directories;
use parking_lot::{
    MappedRwLockReadGuard,
    MappedRwLockWriteGuard,
    RwLock,
    RwLockReadGuard,
    RwLockWriteGuard,
};
use serde_json::Value;
pub use settings::{
    Settings,
    SettingsProvider,
};
pub use state::{
    State,
    StateProvider,
};
use thiserror::Error;
use tracing::error;

pub type Map = serde_json::Map<String, Value>;

/// Drops any group/other bits from `path`, best effort.
fn tighten_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    let mode = metadata.permissions().mode();
    if mode & 0o077 != 0
        && let Err(err) = fs::set_permissions(path, fs::Permissions::from_mode(mode & !0o077))
    {
        error!(%err, ?path, "Failed to tighten permissions");
    }
}

static SETTINGS_FILE_LOCK: RwLock<()> = RwLock::new(());

static SETTINGS_DATA: RwLock<Option<Map>> = RwLock::new(None);

#[derive(Debug, Clone)]
pub enum Backend {
    Global,
    Memory(Map),
}

pub enum ReadGuard<'a, T> {
    Global(RwLockReadGuard<'a, Option<T>>),
    Memory(&'a T),
}

impl<'a, T> ReadGuard<'a, T> {
    fn try_map<U, F: FnOnce(&T) -> Option<&U>>(self, f: F) -> Option<MappedReadGuard<'a, U>> {
        match self {
            ReadGuard::Global(guard) => RwLockReadGuard::<'a, Option<T>>::try_map(guard, |data: &Option<T>| {
                f(data.as_ref().expect("global backend is not used"))
            })
            .ok()
            .map(MappedReadGuard::Global),
            ReadGuard::Memory(data) => f(data).map(MappedReadGuard::Memory),
        }
    }
}

impl<T> std::ops::Deref for ReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            ReadGuard::Global(guard) => guard.as_ref().expect("global backend is not used"),
            ReadGuard::Memory(data) => data,
        }
    }
}

pub enum MappedReadGuard<'a, T> {
    Global(MappedRwLockReadGuard<'a, T>),
    Memory(&'a T),
}

impl<T> std::ops::Deref for MappedReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            MappedReadGuard::Global(guard) => guard,
            MappedReadGuard::Memory(data) => data,
        }
    }
}

pub enum WriteGuard<'a, T> {
    Global(RwLockWriteGuard<'a, Option<T>>),
    Memory(&'a mut T),
}

impl<'a, T> WriteGuard<'a, T> {
    fn try_map<U, F: FnOnce(&mut T) -> Option<&mut U>>(self, f: F) -> Option<MappedWriteGuard<'a, U>> {
        match self {
            WriteGuard::Global(guard) => RwLockWriteGuard::<'a, Option<T>>::try_map(guard, |data: &mut Option<T>| {
                f(data.as_mut().expect("global backend is not used"))
            })
            .ok()
            .map(MappedWriteGuard::Global),
            WriteGuard::Memory(data) => f(data).map(MappedWriteGuard::Memory),
        }
    }
}

impl<T> std::ops::Deref for WriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            WriteGuard::Global(guard) => guard.as_ref().expect("global backend is not used"),
            WriteGuard::Memory(data) => data,
        }
    }
}

impl<T> std::ops::DerefMut for WriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            WriteGuard::Global(guard) => guard.as_mut().expect("global backend is not used"),
            WriteGuard::Memory(data) => data,
        }
    }
}

pub enum MappedWriteGuard<'a, T> {
    Global(MappedRwLockWriteGuard<'a, T>),
    Memory(&'a mut T),
}

impl<T> std::ops::Deref for MappedWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            MappedWriteGuard::Global(guard) => guard,
            MappedWriteGuard::Memory(data) => data,
        }
    }
}

impl<T> std::ops::DerefMut for MappedWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            MappedWriteGuard::Global(guard) => guard,
            MappedWriteGuard::Memory(data) => data,
        }
    }
}

pub trait JsonStore: Sized {
    /// Path to the file
    fn path() -> Result<PathBuf>;

    /// In mem lock on the file
    fn file_lock() -> &'static RwLock<()>;

    /// [RwLock] on the data, [None] if not using the global backend
    fn data_lock() -> &'static RwLock<Option<Map>>;

    fn new_from_backend(backend: Backend) -> Self;

    fn map(&self) -> ReadGuard<'_, Map>;

    fn map_mut(&mut self) -> WriteGuard<'_, Map>;

    fn load() -> Result<Self> {
        let is_global = Self::data_lock().read().as_ref().is_some();
        if is_global {
            Ok(Self::new_from_backend(Backend::Global))
        } else {
            Ok(Self::new_from_backend(Backend::Memory(Self::load_from_file()?)))
        }
    }

    fn load_from_file() -> Result<Map> {
        let path = Self::path()?;

        // If the folder doesn't exist, create it.
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent)?;
        }

        let json: Map = {
            let _lock_guard = Self::file_lock().write();

            // If the file doesn't exist, create it.
            if !path.exists() {
                let mut file = FileRwLock::new(
                    File::options()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .mode(0o600)
                        .open(&path)?,
                );
                file.write()?.write_all(b"{}")?;
                serde_json::Map::new()
            } else {
                // This file holds the user's settings and, for `state`, tokens; the first-creation
                // path used a plain `File::create`, so an install that predates the `mode(0o600)`
                // in `save_to_file` still has a world-readable file until something writes to it.
                tighten_permissions(&path);

                let mut file = FileRwLock::new(File::open(&path)?);
                let mut read = file.write()?;
                serde_json::from_reader(&mut *read)?
            }
        };

        Ok(json)
    }

    /// Loads data from file into global backend
    fn load_into_global() -> Result<()> {
        match Self::load_from_file() {
            Ok(json) => {
                *Self::data_lock().write() = Some(json);
                Ok(())
            },
            Err(err) => {
                *Self::data_lock().write() = Some(Map::new());

                let file_content: Result<String> = (|| {
                    let _lock_guard = Self::file_lock().write();
                    let mut file = FileRwLock::new(File::open(Self::path()?)?);
                    let mut read = file.write()?;
                    let mut content = String::new();
                    #[allow(clippy::verbose_file_reads)]
                    read.read_to_string(&mut content)?;
                    Ok(content)
                })();

                error!(%err, ?file_content, "Failed to load json file into global backend");

                // Move the unreadable file aside instead of writing `{}` over it. It used to be
                // overwritten with defaults here, so a single truncated write — or a JSON file
                // this version cannot parse — silently destroyed every setting the user had, with
                // no copy left anywhere.
                if let Err(err) = Self::quarantine_unparseable_file() {
                    error!(%err, "Failed to move the unparseable json file aside");
                }

                Err(err)
            },
        }
    }

    /// Renames the current file out of the way as `<name>.corrupt-<timestamp>`.
    fn quarantine_unparseable_file() -> Result<()> {
        let path = Self::path()?;
        let Some(name) = path.file_name() else {
            return Ok(());
        };

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();

        let mut quarantined = name.to_os_string();
        quarantined.push(format!(".corrupt-{stamp}"));
        let quarantined = path.with_file_name(quarantined);

        let _lock_guard = Self::file_lock().write();
        fs::rename(&path, &quarantined)?;
        error!(from = ?path, to = ?quarantined, "Moved the unparseable json file aside");
        Ok(())
    }

    fn save_to_file(&self) -> Result<()> {
        let path = Self::path()?;

        // If the folder doesn't exist, create it.
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent)?;
        }

        // Serialize before touching the file at all. The old code opened the real file with
        // `O_TRUNC` and only then tried to serialize, so a failure there left a `{}` file behind.
        let contents = serde_json::to_vec_pretty(&*self.map())?;

        let _lock_guard = Self::file_lock().write();

        // Hold the cross-process advisory lock on the real file for the whole replacement, and
        // open it *without* `O_TRUNC`: the old code truncated before it acquired the lock, so a
        // concurrent reader that won that race read an empty file and saw every setting vanish.
        let mut lock_file = FileRwLock::new(
            File::options()
                .create(true)
                .read(true)
                .write(true)
                // Explicitly *not* truncating: this handle only exists to hold the lock.
                .truncate(false)
                .mode(0o600)
                .open(&path)?,
        );
        let _file_guard = lock_file.write()?;

        let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".tmp");
        let tmp_path = path.with_file_name(tmp_name);

        let write = || -> Result<()> {
            let mut tmp = File::options()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)?;
            tmp.write_all(&contents)?;
            // `rename` orders the directory entry, not the data behind it.
            tmp.sync_all()?;
            drop(tmp);
            fs::rename(&tmp_path, &path)?;
            Ok(())
        };

        write().inspect_err(|_| {
            let _ = fs::remove_file(&tmp_path);
        })
    }

    fn set(&mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) {
        self.map_mut().insert(key.into(), value.into());
    }

    fn get(&self, key: impl AsRef<str>) -> Option<MappedReadGuard<'_, Value>> {
        self.map().try_map(|data| data.get(key.as_ref()))
    }

    fn remove(&mut self, key: impl AsRef<str>) -> Option<Value> {
        self.map_mut().remove(key.as_ref())
    }

    fn get_mut(&mut self, key: impl Into<String>) -> Option<MappedWriteGuard<'_, Value>> {
        self.map_mut().try_map(|data| data.get_mut(&key.into()))
    }

    fn get_bool(&self, key: impl AsRef<str>) -> Option<bool> {
        self.get(key).and_then(|value| value.as_bool())
    }

    fn get_bool_or(&self, key: impl AsRef<str>, default: bool) -> bool {
        self.get_bool(key).unwrap_or(default)
    }

    fn get_string(&self, key: impl AsRef<str>) -> Option<String> {
        self.get(key).and_then(|value| value.as_str().map(|s| s.into()))
    }

    fn get_string_or(&self, key: impl AsRef<str>, default: String) -> String {
        self.get_string(key).unwrap_or(default)
    }

    fn get_int(&self, key: impl AsRef<str>) -> Option<i64> {
        self.get(key).and_then(|value| value.as_i64())
    }

    fn get_int_or(&self, key: impl AsRef<str>, default: i64) -> i64 {
        self.get_int(key).unwrap_or(default)
    }
}

pub struct OldSettings {
    inner: Backend,
}

impl JsonStore for OldSettings {
    fn path() -> Result<PathBuf> {
        Ok(directories::settings_path()?)
    }

    fn file_lock() -> &'static RwLock<()> {
        &SETTINGS_FILE_LOCK
    }

    fn data_lock() -> &'static RwLock<Option<Map>> {
        &SETTINGS_DATA
    }

    fn new_from_backend(backend: Backend) -> Self {
        match backend {
            Backend::Global => Self { inner: Backend::Global },
            Backend::Memory(map) => Self {
                inner: Backend::Memory(map),
            },
        }
    }

    fn map(&self) -> ReadGuard<'_, Map> {
        match &self.inner {
            Backend::Global => ReadGuard::Global(Self::data_lock().read()),
            Backend::Memory(map) => ReadGuard::Memory(map),
        }
    }

    fn map_mut(&mut self) -> WriteGuard<'_, Map> {
        match &mut self.inner {
            Backend::Global => WriteGuard::Global(Self::data_lock().write()),
            Backend::Memory(map) => WriteGuard::Memory(map),
        }
    }
}

#[cfg(test)]
mod test {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::LazyLock;

    use super::*;

    /// A `JsonStore` backed by a file in a temp directory, so the round trip can be checked
    /// without touching the user's real settings. Every nextest test runs in its own process, so
    /// the statics below are private to one test.
    static TEST_DIR: LazyLock<tempfile::TempDir> = LazyLock::new(|| tempfile::tempdir().unwrap());
    static TEST_FILE_LOCK: RwLock<()> = RwLock::new(());
    static TEST_DATA: RwLock<Option<Map>> = RwLock::new(None);

    struct TestStore {
        inner: Backend,
    }

    impl JsonStore for TestStore {
        fn path() -> Result<PathBuf> {
            Ok(TEST_DIR.path().join("local.json"))
        }

        fn file_lock() -> &'static RwLock<()> {
            &TEST_FILE_LOCK
        }

        fn data_lock() -> &'static RwLock<Option<Map>> {
            &TEST_DATA
        }

        fn new_from_backend(backend: Backend) -> Self {
            Self { inner: backend }
        }

        fn map(&self) -> ReadGuard<'_, Map> {
            match &self.inner {
                Backend::Global => ReadGuard::Global(Self::data_lock().read()),
                Backend::Memory(map) => ReadGuard::Memory(map),
            }
        }

        fn map_mut(&mut self) -> WriteGuard<'_, Map> {
            match &mut self.inner {
                Backend::Global => WriteGuard::Global(Self::data_lock().write()),
                Backend::Memory(map) => WriteGuard::Memory(map),
            }
        }
    }

    fn contents() -> String {
        fs::read_to_string(TestStore::path().unwrap()).unwrap()
    }

    fn mode() -> u32 {
        fs::metadata(TestStore::path().unwrap()).unwrap().permissions().mode() & 0o777
    }

    /// The round trip the commented-out `test_store_type` used to cover, against the current API.
    #[test]
    fn set_get_remove_round_trip() {
        let path = TestStore::path().unwrap();

        let mut store = TestStore::load().unwrap();
        assert_eq!(contents(), "{}", "load must create the file");
        assert_eq!(*store.map(), Map::new());

        store.set("a", 123);
        store.set("b", "hello");
        store.set("c", false);
        store.save_to_file().unwrap();
        assert_eq!(contents(), "{\n  \"a\": 123,\n  \"b\": \"hello\",\n  \"c\": false\n}");

        let reloaded = TestStore::load().unwrap();
        assert_eq!(reloaded.get("b").unwrap().as_str(), Some("hello"));
        assert_eq!(reloaded.get_int("a"), Some(123));
        assert_eq!(reloaded.get_bool("c"), Some(false));

        let mut store = TestStore::load().unwrap();
        store.remove("a").unwrap();
        store.save_to_file().unwrap();
        assert_eq!(contents(), "{\n  \"b\": \"hello\",\n  \"c\": false\n}");

        // No temp file left behind by the atomic replacement.
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    /// The file holds settings and, for `state`, tokens. The first-creation path used a plain
    /// `File::create`, which gave 0644 — measurably so on this machine before the fix — and
    /// nothing ever narrowed it again.
    #[test]
    fn the_file_is_never_world_readable() {
        let path = TestStore::path().unwrap();

        TestStore::load().unwrap();
        assert_eq!(mode(), 0o600, "freshly created file");

        // An existing file created by an older version is repaired on the next load.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        TestStore::load().unwrap();
        assert_eq!(mode(), 0o600, "pre-existing 0644 file");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let mut store = TestStore::load().unwrap();
        store.set("a", 1);
        store.save_to_file().unwrap();
        assert_eq!(mode(), 0o600, "after a save");
    }

    /// A file this version cannot parse used to be overwritten with `{}` — every setting the user
    /// had, gone, with no copy anywhere. It must be moved aside instead.
    #[test]
    fn an_unparseable_file_is_preserved() {
        let path = TestStore::path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "invalid json").unwrap();

        TestStore::load_into_global().unwrap_err();

        let quarantined: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().into_string().ok()?;
                name.contains(".corrupt-").then_some(entry.path())
            })
            .collect();

        assert_eq!(quarantined.len(), 1, "the unparseable file was not preserved");
        assert_eq!(fs::read_to_string(&quarantined[0]).unwrap(), "invalid json");
        assert!(!path.exists(), "the unparseable file was left in place");

        // The in-memory backend falls back to empty so the process can keep running.
        assert_eq!(*TEST_DATA.read(), Some(Map::new()));
    }
}
