//! Die vier gemeinsamen Verträge gegen die Speicher-Datenbank.
//!
//! Dieselben Makros laufen später gegen SQLite (P2-10) und SQL Server (P2-15); dort kommen
//! `reset` und – bei SQL Server – eine Sperre dazu.

nullradix_core::board_database_contract!(in_memory, || async {
    nullradix_core::board::in_memory::InMemoryBoardDatabase::new()
});

nullradix_core::question_repository_contract!(in_memory_questions, || async {
    nullradix_core::board::in_memory::InMemoryBoardDatabase::new()
});

nullradix_core::dispatch_run_contract!(in_memory_runs, || async {
    nullradix_core::board::in_memory::InMemoryBoardDatabase::new()
});

nullradix_core::search_contract!(in_memory_search, || async {
    nullradix_core::board::in_memory::InMemoryBoardDatabase::new()
});
