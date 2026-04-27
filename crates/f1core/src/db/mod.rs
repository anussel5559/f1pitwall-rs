pub mod models;
mod queries;
pub mod schema;
mod upserts;

pub use models::*;

use anyhow::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Returns the default centralized database path.
///
/// Uses the OS data directory (`~/.local/share/f1-pitwall/f1-pitwall.db` on Linux,
/// `~/Library/Application Support/f1-pitwall/f1-pitwall.db` on macOS,
/// `AppData\Roaming\f1-pitwall\f1-pitwall.db` on Windows).
pub fn default_db_path() -> PathBuf {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("f1-pitwall");
    dir.join("f1-pitwall.db")
}

pub struct Db {
    pub conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(schema::CREATE_TABLES)?;
        // Migrations for existing databases
        let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN gmt_offset TEXT;");
        let _ = conn.execute_batch("ALTER TABLE sessions ADD COLUMN replay_position TEXT;");
        let _ = conn.execute_batch("ALTER TABLE pit_stops ADD COLUMN date TEXT;");
        // Migrate positions/intervals to history tables (PK now includes date).
        // Only drop if the old schema (PK without date) is detected.
        let needs_migrate: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('positions') WHERE name='date' AND pk > 0",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            == 0;
        if needs_migrate {
            let _ = conn.execute_batch(
                "
                DROP TABLE IF EXISTS positions;
                DROP TABLE IF EXISTS intervals;
            ",
            );
        }
        conn.execute_batch(schema::CREATE_TABLES)?;
        // Seed compound allocations if the table is empty.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM compound_allocations", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        if count == 0 {
            Self::seed_compound_allocations(&conn);
        }
        Ok(Self { conn })
    }

    fn seed_compound_allocations(conn: &Connection) {
        #[derive(serde::Deserialize)]
        struct Row {
            year: i64,
            circuit: String,
            hard: String,
            medium: String,
            soft: String,
        }
        let json = include_str!("../../../../data/compound_allocations.json");
        let rows: Vec<Row> = match serde_json::from_str(json) {
            Ok(r) => r,
            Err(_) => return,
        };
        let _ = conn.execute_batch("BEGIN");
        for r in &rows {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO compound_allocations (year, circuit, hard, medium, soft) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![r.year, r.circuit, r.hard, r.medium, r.soft],
            );
        }
        let _ = conn.execute_batch("COMMIT");
    }

    /// Open an ephemeral in-memory database with the schema applied. For tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(schema::CREATE_TABLES)?;
        Ok(Self { conn })
    }

    /// Open a read-only connection to an existing database.
    /// Skips migrations and table creation — assumes the writer has already set up the schema.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.execute_batch("PRAGMA busy_timeout=5000;")?;
        Ok(Self { conn })
    }

    pub fn begin(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }
}
