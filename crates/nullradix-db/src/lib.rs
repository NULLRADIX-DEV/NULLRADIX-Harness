//! Datenzugriff von NULLRADIX: liest und schreibt die unveränderten Schemata von
//! SQLite (`board.db`) und SQL Server.
//!
//! Die Verträge selbst (Trait `BoardDatabase`, die 17 Repository-Traits, `BoardUnitOfWork`)
//! liegen in `nullradix-core`; dieses Crate hängt davon ab und gibt sie weiter, damit Aufrufer
//! alles über `nullradix_db::…` beziehen können. Umgekehrt darf `nullradix-core` nie von `nullradix-db`
//! abhängen – sonst entstünde ein Zyklus, sobald die Dienste des Fachkerns den Trait brauchen.

pub mod factory;
pub mod managed;
pub mod mssql;
pub mod row_groups;
pub mod runtime;
pub mod schema_version_guard;
pub mod sql_chunks;
pub mod sqlite;

pub use factory::{
    open_checked, open_checked_owned, open_checked_with, AnyBoardDatabase, BoardDatabaseFactory,
    OpenedBoard,
};
pub use managed::{ConnectOptions, ManagedBoardDatabase, DEFAULT_CONNECTION_TIMEOUT_MS};
pub use row_groups::RowGroups;
pub use runtime::BoardRuntime;
pub use schema_version_guard::{
    SchemaVersionGuard, EXPECTED_SCHEMA_VERSION, REQUIRED_SCHEMA_FEATURES,
};
pub use sql_chunks::{SqlChunks, SQL_CHUNK_SIZE};

// Die Verträge aus dem Fachkern samt der Speicher-Datenbank für Tests.
pub use nullradix_core::board::database::*;
pub use nullradix_core::board::in_memory::InMemoryBoardDatabase;

/// Name des Crates, wie er in Protokollen und Diagnoseausgaben erscheint.
pub const CRATE_NAME: &str = "nullradix-db";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_name_ist_gesetzt() {
        assert_eq!(CRATE_NAME, "nullradix-db");
    }

    #[test]
    fn fachkern_ist_eingebunden() {
        assert_eq!(nullradix_core::CRATE_NAME, "nullradix-core");
    }

    #[test]
    fn die_vertrage_des_fachkerns_sind_uber_nullradix_db_erreichbar() {
        // Die Weitergabe mit `pub use` ist Vertrag: P2-5 und P2-11 beziehen alles über nullradix_db
        let filter = crate::TicketFilter::default();
        assert!(!filter.without_texts);
        fn nimmt_eine_datenbank<D: crate::BoardDatabase>(_db: &D) {}
        let _ = nimmt_eine_datenbank::<DummyNiemals>;
    }

    /// Nur als Typ gebraucht, nie gebaut – zeigt, dass der Trait über nullradix_db ansprechbar ist
    enum DummyNiemals {}

    impl crate::BoardDatabase for DummyNiemals {
        async fn transaction<T, W>(&self, _work: W) -> nullradix_core::Result<T>
        where
            T: Send + 'static,
            W: crate::TransactionWork<T> + 'static,
        {
            match *self {}
        }

        async fn change_marker(&self, _board_id: i32) -> nullradix_core::Result<String> {
            match *self {}
        }

        async fn question_marker(&self, _project_id: i32) -> nullradix_core::Result<String> {
            match *self {}
        }

        async fn close(&self) -> nullradix_core::Result<()> {
            match *self {}
        }
    }
}
