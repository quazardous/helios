//! The records of workshop sessions (`docs/design/workshops-on-the-fly.md`,
//! section 2): one document per session in `workshops/sessions`, keyed by
//! the session's id. The vault only keeps them; what they hold is the
//! desktop's (`helios_template::session`). Beside them, each workshop
//! version's decision book in `workshops/books` (`helios_template::run`).

use serde_json::Value;

use crate::{Store, Vault};

/// The namespace of the session records.
pub const SESSIONS: &str = "workshops/sessions";

/// The namespace of the workshops' decision books: one document per
/// workshop version (`id@version`), its runs' junction answers.
pub const BOOKS: &str = "workshops/books";

impl<S: Store> Vault<S> {
    /// A session's record, by its id.
    pub fn workshop_session(&self, id: &str) -> Result<Option<Value>, String> {
        self.store
            .lock()
            .expect("store lock")
            .document(SESSIONS, id)
    }

    /// Every session's record, by id.
    pub fn workshop_sessions(&self) -> Result<Vec<(String, Value)>, String> {
        self.store.lock().expect("store lock").documents(SESSIONS)
    }

    /// Writes (or with `None` drops) a session's record.
    pub fn set_workshop_session(&self, id: &str, record: Option<&Value>) -> Result<(), String> {
        self.transaction(|store| store.set_document(SESSIONS, id, record))
    }

    /// A workshop version's decision book (`workshop`: id@version).
    pub fn workshop_book(&self, workshop: &str) -> Result<Option<Value>, String> {
        self.store
            .lock()
            .expect("store lock")
            .document(BOOKS, workshop)
    }

    /// Writes a workshop version's decision book.
    pub fn set_workshop_book(&self, workshop: &str, book: &Value) -> Result<(), String> {
        self.transaction(|store| store.set_document(BOOKS, workshop, Some(book)))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{MemoryStore, Vault};

    #[test]
    fn a_session_record_is_written_read_and_dropped() {
        let vault = Vault::new(MemoryStore::default());
        let record = json!({ "session": "s-1", "kept": false, "steps": [] });
        vault.set_workshop_session("s-1", Some(&record)).unwrap();
        assert_eq!(vault.workshop_session("s-1").unwrap(), Some(record));
        assert_eq!(vault.workshop_sessions().unwrap().len(), 1);
        vault.set_workshop_session("s-1", None).unwrap();
        assert_eq!(vault.workshop_session("s-1").unwrap(), None);
        assert!(vault.workshop_sessions().unwrap().is_empty());
    }
}
