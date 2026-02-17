//! Kernel — durable state for AgentOS.
//!
//! Three pieces of nuclear-proof state:
//! - Thread table (call stack)
//! - Context store (VMM)
//! - Message journal (audit/tape)
//!
//! One WAL, atomic ops. Everything else is ephemeral userspace.

pub mod context_store;
pub mod error;
pub mod journal;
pub mod thread_table;
pub mod wal;

use std::path::{Path, PathBuf};

use context_store::ContextStore;
use error::KernelResult;
use journal::Journal;
use thread_table::ThreadTable;
use wal::Wal;

/// The kernel: wraps all three stores and provides atomic cross-store operations.
pub struct Kernel {
    pub wal: Wal,
    pub threads: ThreadTable,
    pub contexts: ContextStore,
    pub journal: Journal,
    data_dir: PathBuf,
}

impl Kernel {
    /// Open or create the kernel at the given data directory.
    /// Replays the WAL to recover any uncommitted state.
    pub fn open(data_dir: &Path) -> KernelResult<Self> {
        std::fs::create_dir_all(data_dir)?;

        let wal = Wal::open(&data_dir.join("kernel.wal"))?;
        let mut threads = ThreadTable::open(&data_dir.join("threads.bin"))?;
        let mut contexts = ContextStore::open(&data_dir.join("contexts"))?;
        let mut journal = Journal::open(&data_dir.join("journal.bin"))?;

        // Replay WAL and apply any entries not yet reflected in state
        let entries = wal.replay()?;
        for entry in &entries {
            threads.apply_wal_entry(entry);
            contexts.apply_wal_entry(entry);
            journal.apply_wal_entry(entry);
        }

        Ok(Self {
            wal,
            threads,
            contexts,
            journal,
            data_dir: data_dir.to_path_buf(),
        })
    }

    /// Initialize the root thread with WAL logging.
    pub fn initialize_root(&mut self, organism_name: &str, profile: &str) -> KernelResult<String> {
        let uuid = self.threads.initialize_root(organism_name, profile);
        let entry = self
            .threads
            .wal_entry_initialize_root(&uuid, organism_name, profile);
        self.wal.append(&entry)?;
        Ok(uuid)
    }

    /// Atomic prune: thread pruned + context released + journal updated.
    pub fn prune_thread(
        &mut self,
        thread_id: &str,
    ) -> KernelResult<Option<thread_table::PruneResult>> {
        // Look up what we'll prune before writing WAL
        let prune_result = self.threads.peek_prune(thread_id);
        if prune_result.is_none() {
            return Ok(None);
        }

        // Build batch
        let batch = vec![
            wal::WalEntry::new(wal::EntryType::ThreadPrune, thread_id.as_bytes().to_vec()),
            wal::WalEntry::new(
                wal::EntryType::ContextRelease,
                thread_id.as_bytes().to_vec(),
            ),
            wal::WalEntry::new(
                wal::EntryType::JournalDelivered,
                thread_id.as_bytes().to_vec(),
            ),
        ];

        // WAL first, then apply to state
        self.wal.append_batch(&batch)?;
        let result = self.threads.prune_for_response(thread_id);
        self.contexts.release(thread_id)?;
        self.journal.mark_delivered_by_thread(thread_id);

        Ok(result)
    }

    /// Atomic dispatch: extend thread + allocate context + log journal entry.
    /// Returns the new thread UUID.
    pub fn dispatch_message(
        &mut self,
        from: &str,
        to: &str,
        thread_id: &str,
        message_id: &str,
    ) -> KernelResult<String> {
        // Build batch payload
        let mut dispatch_payload = Vec::new();
        dispatch_payload.extend_from_slice(thread_id.as_bytes());
        dispatch_payload.push(0); // null separator
        dispatch_payload.extend_from_slice(to.as_bytes());

        let mut journal_payload = Vec::new();
        journal_payload.extend_from_slice(message_id.as_bytes());
        journal_payload.push(0);
        journal_payload.extend_from_slice(thread_id.as_bytes());
        journal_payload.push(0);
        journal_payload.extend_from_slice(from.as_bytes());
        journal_payload.push(0);
        journal_payload.extend_from_slice(to.as_bytes());

        let batch = vec![
            wal::WalEntry::new(wal::EntryType::ThreadExtend, dispatch_payload),
            wal::WalEntry::new(
                wal::EntryType::ContextAllocate,
                thread_id.as_bytes().to_vec(),
            ),
            wal::WalEntry::new(wal::EntryType::JournalDispatched, journal_payload),
        ];

        self.wal.append_batch(&batch)?;

        let new_uuid = self.threads.extend_chain(thread_id, to);
        self.contexts.create(thread_id)?;
        self.journal
            .log_dispatch_simple(message_id, thread_id, from, to);

        Ok(new_uuid)
    }

    /// Get a reference to the thread table.
    pub fn threads(&self) -> &ThreadTable {
        &self.threads
    }

    /// Get a mutable reference to the thread table.
    pub fn threads_mut(&mut self) -> &mut ThreadTable {
        &mut self.threads
    }

    /// Get a reference to the context store.
    pub fn contexts(&self) -> &ContextStore {
        &self.contexts
    }

    /// Get a mutable reference to the context store.
    pub fn contexts_mut(&mut self) -> &mut ContextStore {
        &mut self.contexts
    }

    /// Get a reference to the journal.
    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// Get a reference to the WAL.
    pub fn wal(&self) -> &Wal {
        &self.wal
    }

    /// Data directory path.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn kernel_opens_and_creates_dirs() {
        let dir = TempDir::new().unwrap();
        let _kernel = Kernel::open(&dir.path().join("data")).unwrap();
        assert!(dir.path().join("data").exists());
        assert!(dir.path().join("data/kernel.wal").exists());
    }

    #[test]
    fn kernel_dispatch_and_prune_lifecycle() {
        let dir = TempDir::new().unwrap();
        let mut kernel = Kernel::open(&dir.path().join("data")).unwrap();

        // Initialize root thread
        let root = kernel.initialize_root("test", "root").unwrap();

        // Dispatch: extends chain root → handler
        let new_uuid = kernel
            .dispatch_message("console", "handler", &root, "msg-001")
            .unwrap();

        assert!(kernel.threads().lookup(&new_uuid).is_some());
        assert!(kernel.contexts().exists(&root));

        // Prune: handler responds
        let prune = kernel.prune_thread(&new_uuid).unwrap();
        assert!(prune.is_some());
    }

    #[test]
    fn kernel_crash_recovery() {
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("data");

        // First session: create some state
        {
            let mut kernel = Kernel::open(&data_dir).unwrap();
            kernel.initialize_root("test", "root").unwrap();
        }

        // Second session: reopen — WAL replay should recover state
        let kernel = Kernel::open(&data_dir).unwrap();
        // The root should exist (either from mmap or WAL replay)
        assert!(kernel.threads().root_uuid().is_some());
    }
}
