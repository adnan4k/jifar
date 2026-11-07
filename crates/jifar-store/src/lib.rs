//! SQLite storage on a dedicated thread, numbered migrations and FTS5 (SPEC §7).
//!
//! Schema changes only via new numbered migrations; never edit an applied one.
//! FTS uses `tokenchars "'"` so Qubee words like `ba'e` stay whole.
