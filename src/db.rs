//! SQLite output: one row per run in the `runs` table.

use crate::{Config, RunResult};
use rusqlite::{Connection, params};
use std::path::Path;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS runs (
    id                INTEGER PRIMARY KEY,
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
        if conn.prepare("SELECT regressions, peak_fitness FROM runs LIMIT 0").is_err() {
            return Err("database has an outdated `runs` table; use a new database file".into());
        }
        Ok(Self { conn })
    }

    /// Inserts finished runs in a single transaction.
    pub fn record_runs(&mut self, records: &[RunRecord]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO runs (started_at, duration_ms, seed, target, target_length, charset,
                                   charset_size, offspring, mutation_rate, max_generations,
                                   generations, reached_target, initial_fitness, final_fitness,
                                   peak_fitness, regressions)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            )?;
            for RunRecord { config, seed, started_at_ms, duration_ms, result } in records {
                stmt.execute(params![
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
    fn records_runs() {
        let mut db = Database::open(":memory:").unwrap();
        let config = Config::default();
        let records: Vec<_> = (0..3)
            .map(|seed| {
                let mut rng = StdRng::seed_from_u64(seed);
                let result = run(&config, &mut rng, |_| {}).unwrap();
                RunRecord { config: config.clone(), seed, started_at_ms: 0, duration_ms: 1.0, result }
            })
            .collect();

        db.record_runs(&records).unwrap();

        let rows: Vec<(String, i64, bool, i64)> = db
            .conn
            .prepare("SELECT seed, generations, reached_target, regressions FROM runs ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 3);
        for ((seed, gens, reached, regressions), rec) in rows.into_iter().zip(&records) {
            assert_eq!(seed, rec.seed.to_string());
            assert_eq!(gens as u64, rec.result.generations);
            assert!(reached);
            assert_eq!(regressions as u64, rec.result.regressions);
        }
    }
}
