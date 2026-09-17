//! SQLite output: one row per invocation in `batches`, one row per run in `runs`.

use crate::{Config, RunResult};
use rusqlite::{Connection, params};
use std::path::Path;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS batches (
    id                INTEGER PRIMARY KEY,
    label             TEXT,
    command           TEXT    NOT NULL, -- full command line
    seed              TEXT    NOT NULL, -- base seed; run i of each combination uses seed + i
    runs_per_combo    INTEGER NOT NULL,
    combos            INTEGER NOT NULL,
    started_at        INTEGER NOT NULL, -- unix epoch milliseconds
    finished_at       INTEGER           -- NULL if the batch was interrupted or failed
);
CREATE TABLE IF NOT EXISTS runs (
    id                INTEGER PRIMARY KEY,
    batch_id          INTEGER NOT NULL REFERENCES batches(id),
    started_at        INTEGER NOT NULL, -- unix epoch milliseconds
    duration_ms       REAL    NOT NULL,
    seed              TEXT    NOT NULL, -- u64, stored as text since it may exceed i64
    target            TEXT    NOT NULL,
    target_length     INTEGER NOT NULL,
    charset           TEXT    NOT NULL,
    charset_size      INTEGER NOT NULL,
    offspring         INTEGER NOT NULL,
    mutation_rate     REAL    NOT NULL,
    max_generations   INTEGER,
    generations       INTEGER NOT NULL,
    reached_target    INTEGER NOT NULL, -- 0/1
    initial_fitness   INTEGER NOT NULL,
    final_fitness     INTEGER NOT NULL,
    peak_fitness      INTEGER NOT NULL,
    regressions       INTEGER NOT NULL  -- generations where fitness dropped
);
CREATE INDEX IF NOT EXISTS runs_batch_id ON runs(batch_id);
";

/// A finished run, ready to be stored.
#[derive(Debug, Clone)]
pub struct RunRecord {
    pub config: Config,
    pub seed: u64,
    pub started_at_ms: i64,
    pub duration_ms: f64,
    pub result: RunResult,
}

/// Describes one invocation of the program.
#[derive(Debug, Clone)]
pub struct BatchInfo<'a> {
    pub label: Option<&'a str>,
    pub command: &'a str,
    pub seed: u64,
    pub runs_per_combo: u64,
    pub combos: usize,
    pub started_at_ms: i64,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let conn = Connection::open(path)?;
        // WAL lets Grafana read while runs are being written.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        // `CREATE TABLE IF NOT EXISTS` leaves an older `runs` table untouched; catch that early.
        if conn.prepare("SELECT batch_id, regressions, peak_fitness FROM runs LIMIT 0").is_err() {
            return Err("database has an outdated `runs` table; use a new database file".into());
        }
        Ok(Self { conn })
    }

    /// Creates a batch and returns its id.
    pub fn begin_batch(&self, info: &BatchInfo) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO batches (label, command, seed, runs_per_combo, combos, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                info.label,
                info.command,
                info.seed.to_string(),
                info.runs_per_combo as i64,
                info.combos as i64,
                info.started_at_ms,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn finish_batch(&self, batch_id: i64, finished_at_ms: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE batches SET finished_at = ?1 WHERE id = ?2",
            params![finished_at_ms, batch_id],
        )?;
        Ok(())
    }

    /// Inserts finished runs belonging to `batch_id` in a single transaction.
    pub fn record_runs(&mut self, batch_id: i64, records: &[RunRecord]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO runs (batch_id, started_at, duration_ms, seed, target, target_length,
                                   charset, charset_size, offspring, mutation_rate, max_generations,
                                   generations, reached_target, initial_fitness, final_fitness,
                                   peak_fitness, regressions)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            )?;
            for RunRecord { config, seed, started_at_ms, duration_ms, result } in records {
                stmt.execute(params![
                    batch_id,
                    started_at_ms,
                    duration_ms,
                    seed.to_string(),
                    config.target.iter().collect::<String>(),
                    config.target.len() as i64,
                    config.charset.iter().collect::<String>(),
                    config.charset.len() as i64,
                    config.offspring as i64,
                    config.mutation_rate,
                    config.max_generations.map(|m| m as i64),
                    result.generations as i64,
                    result.reached_target,
                    result.initial_fitness as i64,
                    result.fitness as i64,
                    result.peak_fitness as i64,
                    result.regressions as i64,
                ])?;
            }
        }
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn records_batch_and_runs() {
        let mut db = Database::open(":memory:").unwrap();
        let config = Config::default();
        let records: Vec<_> = (0..3)
            .map(|seed| {
                let mut rng = StdRng::seed_from_u64(seed);
                let result = run(&config, &mut rng, |_| {}).unwrap();
                RunRecord { config: config.clone(), seed, started_at_ms: 0, duration_ms: 1.0, result }
            })
            .collect();

        let info = BatchInfo {
            label: Some("test"),
            command: "weasel -r 3",
            seed: 0,
            runs_per_combo: 3,
            combos: 1,
            started_at_ms: 10,
        };
        let batch_id = db.begin_batch(&info).unwrap();
        db.record_runs(batch_id, &records).unwrap();

        let finished: Option<i64> = db
            .conn
            .query_row("SELECT finished_at FROM batches WHERE id = ?1", [batch_id], |r| r.get(0))
            .unwrap();
        assert_eq!(finished, None);
        db.finish_batch(batch_id, 20).unwrap();

        let (label, finished): (String, Option<i64>) = db
            .conn
            .query_row("SELECT label, finished_at FROM batches WHERE id = ?1", [batch_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((label.as_str(), finished), ("test", Some(20)));

        let rows: Vec<(i64, String, i64, bool, i64)> = db
            .conn
            .prepare(
                "SELECT batch_id, seed, generations, reached_target, regressions
                 FROM runs ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 3);
        for ((row_batch, seed, gens, reached, regressions), rec) in rows.into_iter().zip(&records) {
            assert_eq!(row_batch, batch_id);
            assert_eq!(seed, rec.seed.to_string());
            assert_eq!(gens as u64, rec.result.generations);
            assert!(reached);
            assert_eq!(regressions as u64, rec.result.regressions);
        }
    }

    #[test]
    fn rejects_outdated_schema() {
        let dir = std::env::temp_dir().join(format!("weasel-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.db");
        let _ = std::fs::remove_file(&path);
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE runs (id INTEGER PRIMARY KEY, seed TEXT)")
            .unwrap();

        assert!(Database::open(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
