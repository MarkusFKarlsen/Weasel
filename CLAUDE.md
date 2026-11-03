# weasel

A Rust implementation of Richard Dawkins' "weasel program" (cumulative selection towards a
target string), built as a **data-generation tool for statistical analysis**. The user runs large
parameter sweeps, stores one row per run in SQLite, and explores the results in **Grafana**
(`frser-sqlite-datasource` plugin).

## Goals

- Measure how generations-to-target depends on the parameters: target length (L), charset size
  (k), offspring per generation (n), and per-character mutation rate (m).
- Store **results only**: settings and outcome per run, never per-generation history. The user
  explicitly removed per-generation storage; don't reintroduce it without asking.
- Reproducible: any run can be recreated from its stored seed and settings.
- Fast: runs execute in parallel across all cores (the dev machine has 32 threads).

## The algorithm (`src/lib.rs`)

Start from a random string. Each generation, make `offspring` copies of the parent. Each
character of each copy is replaced with a random charset character with probability
`mutation_rate` (the replacement may be the same character). The fittest copy (most positions
matching the target; the first one wins ties) becomes the next parent. **The parent does not
compete** (classic Dawkins), so fitness can drop; these drops are counted as `regressions`.
Continue until fitness == target length or `max_generations` is reached.

Defaults: target `"methinks it is like a weasel"`, charset `a-z` plus a space (27 characters),
offspring 100, mutation rate 0.05.

## Layout

- `src/lib.rs`: `Config` (+ `validate`), `run(config, rng, on_generation)` → `RunResult`
  (generations, reached_target, fitness, initial_fitness, peak_fitness, regressions). Pure logic.
- `src/sweep.rs`: `Sweep<T>` CLI parser: comma-separated values and/or inclusive
  `start:end:step` ranges. Float values are rounded to 1e-9 so they group cleanly in SQL.
- `src/db.rs`: SQLite schema, `Database::{open, begin_batch, record_runs, finish_batch}`.
- `src/main.rs`: clap CLI, builds the combination grid, runs a scoped worker-thread pool, and
  writes to the database from the main thread.

## CLI summary

`-t` target | `-l` random target length* | `-c` charset | `-k` charset size (first k chars)* |
`-n` offspring* | `-m` mutation rate* | `--max-generations` | `-s` seed | `-r` runs per
combination | `-j` threads | `--db` path | `--label` (requires --db) | `-q` quiet.
(\* = sweepable; all sweeps form a full cartesian grid.)

Example: `cargo run --release -- --db weasel.db --label "L vs k" -l 5:30:5 -k 2,10,27 -r 200 --max-generations 20000 -q`

## Database schema

- `batches`: one row per invocation: id, label, command, seed (TEXT), runs_per_combo, combos,
  started_at, finished_at (**NULL = interrupted or failed**).
- `runs`: one row per run: batch_id, started_at, duration_ms, seed (TEXT, since a u64 can
  exceed i64), target, target_length, charset, charset_size, offspring, mutation_rate,
  max_generations, generations, reached_target (0/1), initial_fitness, final_fitness,
  peak_fitness, regressions. Indexed on batch_id.
- Timestamps are Unix epoch **milliseconds** (divide by 1000 for Grafana).
- WAL mode + `synchronous=NORMAL` so Grafana can read during writes.
- There are no migrations: `Database::open` rejects outdated schemas and the user starts a new
  database file. Any schema change breaks existing databases, so call that out when proposing one.

## Invariants and design decisions

- **Determinism:** run i of *every* combination uses seed `base_seed + i`, with its own `StdRng`.
  Results must be identical regardless of thread count (verified by comparing `-j 1` against all
  threads). Combinations with the same (L, k) and seed get the same random target, which gives a
  fair comparison across n and m.
- Jobs cycle through combinations (`combo = job % combos`) so a partial or interrupted batch
  covers every combination evenly.
- Only the main thread touches SQLite. Workers send `RunRecord`s over a bounded channel, and the
  main thread writes them in transactions of up to 1000 rows.
- Per-generation console output only when running on 1 thread; otherwise one summary line per run.
- Every combination is validated before anything runs. Errors name the failing combination.
- A warning is printed when sweeping without `--max-generations`: low n with high m can plateau
  below the target forever (e.g. n=10, m≥0.07 never converges; n=5, m=0.1 plateaus around 16/28).

## Analysis caveats (tell the user when relevant)

- Runs that hit the cap are censored: their `generations` equals the cap, so a plain
  `AVG(generations)` is misleading. Report `AVG(reached_target)` alongside it, or compare only
  combinations where every run succeeded.
- `duration_ms` is inflated when runs share the CPU; only compare timings from `-j 1` runs.
- With 100 offspring, `regressions` is almost always 0; it becomes informative at low n.

## Conventions

- Rust edition 2024. Dependencies: `rand` 0.10 (note the API: `rand::{Rng, RngExt}`,
  `rand::seq::IndexedRandom`, `rng.random_bool`), `clap` 4 (derive), `rusqlite` 0.40 (`bundled`).
- Before finishing a change, run `cargo test`, `cargo clippy --all-targets`, and an end-to-end
  check of real runs against a scratch database.
- Commit only when the user asks. Work happens on `master`.

## Known issues and ideas not yet done

- Piping output into something that closes early (`| head`) makes the program panic with
  "Broken pipe" (only a cosmetic problem).
- Possible additions: an index on (offspring, mutation_rate, target_length) for large databases;
  a "parent competes" (elitism) variant flag for comparison with the classic algorithm.
