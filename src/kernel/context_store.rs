//! Context store — per-thread context allocation.
//!
//! Each thread gets its own context buffer. This is the "virtual memory
//! manager" storage layer — without paging intelligence (that's Phase 3's
//! librarian). Just raw allocation, append, read, release.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::error::{KernelError, KernelResult};
use super::wal::{EntryType, WalEntry};

/// Per-thread context storage.
///
/// Stores context data in memory (HashMap-backed). WAL ensures durability.
/// Context files on disk could be added later for large contexts.
#[allow(dead_code)]
pub struct ContextStore {
    /// thread_id → context data
    contexts: HashMap<String, Vec<u8>>,
    /// Base directory for context files
    base_dir: PathBuf,
}

impl ContextStore {
    /// Open or create the context store.
    pub fn open(base_dir: &Path) -> KernelResult<Self> {
        std::fs::create_dir_all(base_dir)?;
        Ok(Self {
            contexts: HashMap::new(),
            base_dir: base_dir.to_path_buf(),
        })
    }

    /// Apply a WAL entry during replay.
    pub fn apply_wal_entry(&mut self, entry: &WalEntry) {
        match entry.entry_type {
            EntryType::ContextAllocate => {
                let thread_id = String::from_utf8_lossy(&entry.payload).to_string();
                self.contexts.entry(thread_id).or_default();
            }
            EntryType::ContextAppend => {
                // Payload: thread_id\0data
                if let Some((thread_id, data)) = Self::parse_append_payload(&entry.payload) {
                    self.contexts
                        .entry(thread_id)
                        .or_default()
                        .extend_from_slice(&data);
                }
            }
            EntryType::ContextRelease => {
                let thread_id = String::from_utf8_lossy(&entry.payload).to_string();
                self.contexts.remove(&thread_id);
            }
            _ => {} // not a context op
        }
    }

    /// Create a context for a thread (allocate).
    pub fn create(&mut self, thread_id: &str) -> KernelResult<()> {
        self.contexts.entry(thread_id.to_string()).or_default();
        Ok(())
    }

    /// Build a WAL entry for context creation.
    pub fn wal_entry_create(thread_id: &str) -> WalEntry {
        WalEntry::new(EntryType::ContextAllocate, thread_id.as_bytes().to_vec())
    }

    /// Get context data for a thread (zero-copy read from HashMap).
    pub fn get(&self, thread_id: &str) -> Option<&[u8]> {
        self.contexts.get(thread_id).map(|v| v.as_slice())
    }

    /// Append data to a thread's context.
    pub fn append(&mut self, thread_id: &str, data: &[u8]) -> KernelResult<()> {
        let ctx = self
            .contexts
            .get_mut(thread_id)
            .ok_or_else(|| KernelError::ContextNotFound(thread_id.to_string()))?;
        ctx.extend_from_slice(data);
        Ok(())
    }

    /// Build a WAL entry for context append.
    pub fn wal_entry_append(thread_id: &str, data: &[u8]) -> WalEntry {
        let mut payload = Vec::new();
        payload.extend_from_slice(thread_id.as_bytes());
        payload.push(0);
        payload.extend_from_slice(data);
        WalEntry::new(EntryType::ContextAppend, payload)
    }

    /// Release (free) a thread's context.
    pub fn release(&mut self, thread_id: &str) -> KernelResult<()> {
        self.contexts.remove(thread_id);
        Ok(())
    }

    /// Build a WAL entry for context release.
    pub fn wal_entry_release(thread_id: &str) -> WalEntry {
        WalEntry::new(EntryType::ContextRelease, thread_id.as_bytes().to_vec())
    }

    /// Check if a context exists for a thread.
    pub fn exists(&self, thread_id: &str) -> bool {
        self.contexts.contains_key(thread_id)
    }

    /// Number of active contexts.
    pub fn count(&self) -> usize {
        self.contexts.len()
    }

    fn parse_append_payload(payload: &[u8]) -> Option<(String, Vec<u8>)> {
        let pos = payload.iter().position(|&b| b == 0)?;
        let thread_id = String::from_utf8_lossy(&payload[..pos]).to_string();
        let data = payload[pos + 1..].to_vec();
        Some((thread_id, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn create_and_read() {
        let dir = TempDir::new().unwrap();
        let mut store = ContextStore::open(&dir.path().join("contexts")).unwrap();

        store.create("thread-1").unwrap();
        assert!(store.exists("thread-1"));
        assert_eq!(store.get("thread-1"), Some(b"".as_slice()));
    }

    #[test]
    fn append_and_read() {
        let dir = TempDir::new().unwrap();
        let mut store = ContextStore::open(&dir.path().join("contexts")).unwrap();

        store.create("thread-1").unwrap();
        store.append("thread-1", b"hello ").unwrap();
        store.append("thread-1", b"world").unwrap();

        assert_eq!(store.get("thread-1"), Some(b"hello world".as_slice()));
    }

    #[test]
    fn release_frees() {
        let dir = TempDir::new().unwrap();
        let mut store = ContextStore::open(&dir.path().join("contexts")).unwrap();

        store.create("thread-1").unwrap();
        store.append("thread-1", b"data").unwrap();
        store.release("thread-1").unwrap();

        assert!(!store.exists("thread-1"));
        assert_eq!(store.get("thread-1"), None);
    }

    #[test]
    fn append_nonexistent_fails() {
        let dir = TempDir::new().unwrap();
        let mut store = ContextStore::open(&dir.path().join("contexts")).unwrap();

        let err = store.append("nonexistent", b"data").unwrap_err();
        assert!(matches!(err, KernelError::ContextNotFound(_)));
    }

    #[test]
    fn wal_replay_recovers_context() {
        let dir = TempDir::new().unwrap();
        let mut store = ContextStore::open(&dir.path().join("contexts")).unwrap();

        // Simulate WAL replay
        store.apply_wal_entry(&WalEntry::new(
            EntryType::ContextAllocate,
            b"thread-1".to_vec(),
        ));

        let mut append_payload = b"thread-1".to_vec();
        append_payload.push(0);
        append_payload.extend_from_slice(b"context data");
        store.apply_wal_entry(&WalEntry::new(EntryType::ContextAppend, append_payload));

        assert_eq!(store.get("thread-1"), Some(b"context data".as_slice()));

        // Release via WAL replay
        store.apply_wal_entry(&WalEntry::new(
            EntryType::ContextRelease,
            b"thread-1".to_vec(),
        ));
        assert!(!store.exists("thread-1"));
    }
}
