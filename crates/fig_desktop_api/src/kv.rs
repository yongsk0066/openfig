use std::sync::Arc;

use dashmap::DashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum KVError {
    #[error("{}", .0)]
    Serde(#[from] serde_json::Error),
}

type Result<T, E = KVError> = std::result::Result<T, E>;

// The store is process-local (see DashKVStore below), so the encoding only has to be
// deterministic within a single run: it never reaches disk or another process. This used to be
// bincode, which is permanently discontinued (RUSTSEC-2025-0141); serde_json is already a
// dependency of this crate and needs no new one.

#[inline(always)]
fn serialize(value: impl serde::Serialize) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&value)?)
}

#[inline(always)]
fn deserialize<T: serde::de::DeserializeOwned>(value: &[u8]) -> Result<T> {
    Ok(serde_json::from_slice(value)?)
}

pub trait KVStore {
    /// Sets the value of a key
    fn set_raw(&self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>);

    /// Gets the value of a key
    fn get_raw(&self, key: impl AsRef<[u8]>) -> Option<Vec<u8>>;

    /// Set a serilizable key/value
    fn set<K: serde::Serialize + ?Sized, V: serde::Serialize + ?Sized>(&self, key: &K, value: &V) -> Result<()> {
        let key = serialize(key)?;
        let value = serialize(value)?;
        self.set_raw(key, value);
        Ok(())
    }

    /// Get a serilizable key/value
    fn get<K: serde::Serialize + ?Sized, V: serde::de::DeserializeOwned>(&self, key: &K) -> Result<Option<V>> {
        let key = serialize(key)?;
        let Some(value) = self.get_raw(key) else {
            return Ok(None);
        };
        let value = deserialize(&value)?;
        Ok(Some(value))
    }
}

/// A [DashMap] wrapper that implements [KVStore]
#[derive(Debug, Default)]
pub struct DashKVStore(DashMap<Vec<u8>, Vec<u8>>);

impl DashKVStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KVStore for DashKVStore {
    fn set_raw(&self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        self.0.insert(key.into(), value.into());
    }

    fn get_raw(&self, key: impl AsRef<[u8]>) -> Option<Vec<u8>> {
        self.0.get(key.as_ref()).as_ref().map(|v| v.to_vec())
    }
}

impl KVStore for Arc<DashKVStore> {
    fn set_raw(&self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        self.0.insert(key.into(), value.into());
    }

    fn get_raw(&self, key: impl AsRef<[u8]>) -> Option<Vec<u8>> {
        self.0.get(key.as_ref()).as_ref().map(|v| v.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashkvstore() {
        let store = DashKVStore::new();

        store.set("foo", "bar").unwrap();
        assert_eq!(store.get("foo").unwrap(), Some("bar".to_string()));
        assert_eq!(store.get::<_, String>("bar").unwrap(), None);

        store.set(&["scoped", "key"], "value").unwrap();
        assert_eq!(store.get(&["scoped", "key"]).unwrap(), Some("value".to_string()));

        store.set("number", &42).unwrap();
        assert_eq!(store.get("number").unwrap(), Some(42));

        store.set("bool", &true).unwrap();
        assert_eq!(store.get("bool").unwrap(), Some(true));

        println!("{store:?}");
    }
}
