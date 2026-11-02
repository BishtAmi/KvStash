use bytes::Bytes;
use kv_types::ChunkKey;
use lru::LruCache;
use std::sync::{Mutex, MutexGuard};
use thiserror::Error;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum BackendError {
    #[error("cache lock was poisoned")]
    Poisoned,
    #[error("item size {item_bytes} exceeds cache capacity {capacity_bytes}")]
    ItemTooLarge {
        item_bytes: u64,
        capacity_bytes: u64,
    },
    #[error("item size cannot be represented as u64")]
    ItemSizeOverflow,
    #[error("cache byte accounting invariant was violated")]
    AccountingInvariant,
}

pub trait StorageBackend: Send + Sync {
    fn contains(&self, key: &ChunkKey) -> Result<bool, BackendError>;
    fn put(&self, key: ChunkKey, value: Bytes) -> Result<(), BackendError>;
    fn get(&self, key: &ChunkKey) -> Result<Option<Bytes>, BackendError>;
    fn delete(&self, key: &ChunkKey) -> Result<bool, BackendError>;
    fn used_bytes(&self) -> Result<u64, BackendError>;
}

struct Inner {
    entries: LruCache<ChunkKey, Bytes>,
    used_bytes: u64,
}

pub struct MemoryBackend {
    inner: Mutex<Inner>,
    capacity_bytes: u64,
}

impl MemoryBackend {
    pub fn new(capacity_bytes: u64) -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: LruCache::unbounded(),
                used_bytes: 0,
            }),
            capacity_bytes,
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Inner>, BackendError> {
        self.inner.lock().map_err(|_| BackendError::Poisoned)
    }

    pub fn capacity_bytes(&self) -> u64 {
        self.capacity_bytes
    }
}

impl StorageBackend for MemoryBackend {
    fn contains(&self, key: &ChunkKey) -> Result<bool, BackendError> {
        Ok(self.lock()?.entries.contains(key))
    }

    fn put(&self, key: ChunkKey, value: Bytes) -> Result<(), BackendError> {
        let item_bytes =
            u64::try_from(value.len()).map_err(|_| BackendError::ItemSizeOverflow)?;
        if item_bytes > self.capacity_bytes {
            return Err(BackendError::ItemTooLarge {
                item_bytes,
                capacity_bytes: self.capacity_bytes,
            });
        }

        let mut inner = self.lock()?;
        if let Some(previous) = inner.entries.pop(&key) {
            inner.used_bytes -= previous.len() as u64;
        }

        let target_usage = self.capacity_bytes - item_bytes;
        while inner.used_bytes > target_usage {
            let (_, evicted) = inner.entries.pop_lru().ok_or(BackendError::AccountingInvariant)?;
            inner.used_bytes -= evicted.len() as u64;
        }

        inner.used_bytes += item_bytes;
        inner.entries.put(key, value);
        Ok(())
    }

    fn get(&self, key: &ChunkKey) -> Result<Option<Bytes>, BackendError> {
        Ok(self.lock()?.entries.get(key).cloned())
    }

    fn delete(&self, key: &ChunkKey) -> Result<bool, BackendError> {
        let mut inner = self.lock()?;
        if let Some(value) = inner.entries.pop(key) {
            inner.used_bytes -= value.len() as u64;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn used_bytes(&self) -> Result<u64, BackendError> {
        Ok(self.lock()?.used_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{BackendError, MemoryBackend, StorageBackend};
    use bytes::Bytes;
    use kv_types::ChunkKey;

    fn key(byte: u8) -> ChunkKey {
        ChunkKey::from_bytes([byte; ChunkKey::LENGTH])
    }

    #[test]
    fn evicts_least_recently_used_entry_to_meet_budget() {
        let backend = MemoryBackend::new(100);
        backend.put(key(1), Bytes::from(vec![1; 60])).unwrap();
        backend.put(key(2), Bytes::from(vec![2; 40])).unwrap();
        backend.get(&key(1)).unwrap();
        backend.put(key(3), Bytes::from(vec![3; 40])).unwrap();

        assert!(backend.contains(&key(1)).unwrap());
        assert!(!backend.contains(&key(2)).unwrap());
        assert!(backend.contains(&key(3)).unwrap());
        assert_eq!(backend.used_bytes().unwrap(), 100);
    }

    #[test]
    fn replacing_an_entry_updates_accounting() {
        let backend = MemoryBackend::new(10);
        backend.put(key(1), Bytes::from_static(b"123456")).unwrap();
        backend.put(key(1), Bytes::from_static(b"abc")).unwrap();
        assert_eq!(backend.used_bytes().unwrap(), 3);
        assert_eq!(backend.get(&key(1)).unwrap().unwrap().as_ref(), b"abc");
    }

    #[test]
    fn rejects_entries_larger_than_the_entire_budget() {
        let backend = MemoryBackend::new(2);
        assert_eq!(
            backend.put(key(1), Bytes::from_static(b"abc")),
            Err(BackendError::ItemTooLarge {
                item_bytes: 3,
                capacity_bytes: 2
            })
        );
        assert_eq!(backend.used_bytes().unwrap(), 0);
    }

    #[test]
    fn zero_capacity_accepts_only_empty_values() {
        let backend = MemoryBackend::new(0);
        backend.put(key(1), Bytes::new()).unwrap();
        assert!(backend.contains(&key(1)).unwrap());
        assert!(backend.put(key(2), Bytes::from_static(b"x")).is_err());
    }

    #[test]
    fn delete_releases_accounted_bytes() {
        let backend = MemoryBackend::new(10);
        backend.put(key(1), Bytes::from_static(b"1234")).unwrap();
        assert!(backend.delete(&key(1)).unwrap());
        assert!(!backend.delete(&key(1)).unwrap());
        assert_eq!(backend.used_bytes().unwrap(), 0);
    }
}
