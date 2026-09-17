use clap::Parser;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use weasel::db::{BatchInfo, Database, RunRecord};
use weasel::sweep::Sweep;
use weasel::{
    Config, DEFAULT_CHARSET, DEFAULT_MUTATION_RATE, DEFAULT_OFFSPRING, DEFAULT_TARGET,
    dedup_charset, random_string, run,
};

/// Maximum number of finished runs written to the database per transaction.
const DB_BATCH_SIZE: usize = 1000;

/// Dawkins' weasel program.
///
/// Options marked "sweepable" accept a comma-separated list of values and/or inclusive
/// start:end:step ranges, e.g. 10:100:10,500. Every combination of swept values is run
/// --runs times.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Target string to evolve towards
    #[arg(short, long, default_value = DEFAULT_TARGET, conflicts_with = "length")]
    target: String,

    /// Use random targets of this length (drawn from the charset) instead of --target. Sweepable
    #[arg(short, long)]
    length: Option<Sweep<usize>>,

    /// Valid characters for mutations (and the target)
    #[arg(short, long, default_value = DEFAULT_CHARSET)]
    charset: String,

    /// Only use the first N characters of --charset. Sweepable [default: all of them]
    #[arg(short = 'k', long)]
    charset_size: Option<Sweep<usize>>,

    /// Mutated copies (offspring) produced per generation. Sweepable
    #[arg(short = 'n', long, default_value_t = Sweep(vec![DEFAULT_OFFSPRING]))]
    offspring: Sweep<usize>,

    /// Per-character mutation probability, in (0, 1]. Sweepable
    #[arg(short, long, default_value_t = Sweep(vec![DEFAULT_MUTATION_RATE]))]
    mutation_rate: Sweep<f64>,

    /// Give up after this many generations
    #[arg(long)]
    max_generations: Option<u64>,

    /// RNG seed for reproducible runs (random if omitted); run i of each combination uses seed + i
    #[arg(short, long)]
    seed: Option<u64>,

    /// Number of independent runs per combination of swept values
    #[arg(short, long, default_value_t = 1)]
    runs: u64,

    /// Worker threads for running in parallel [default: number of CPUs]
    #[arg(short = 'j', long)]
    threads: Option<NonZeroUsize>,

    /// SQLite database to append results to (created if missing)
    #[arg(long)]
    db: Option<PathBuf>,

    /// Label stored with this batch of runs in the database
    #[arg(long, requires = "db")]
    label: Option<String>,

    /// Only print one summary line per run (implied when using more than one thread)
    #[arg(short, long)]
    quiet: bool,
}

/// One combination of swept parameters.
#[derive(Debug, Clone, Copy)]
struct Combo {
    /// Random target length, or `None` to use --target.
    length: Option<usize>,
    charset_size: usize,
    offspring: usize,
    mutation_rate: f64,
}

fn main() -> ExitCode {
    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn try_main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let base_seed = args.seed.unwrap_or_else(rand::random);
    let charset = dedup_charset(args.charset.chars());
    let db = args.db.as_ref().map(Database::open).transpose()?;

    let lengths = match &args.length {
        Some(sweep) => sweep.0.iter().copied().map(Some).collect(),
        None => vec![None],
    };
    let charset_sizes = match &args.charset_size {
        Some(sweep) => sweep.0.clone(),
        None => vec![charset.len()],
    };
    if let Some(&size) = charset_sizes.iter().find(|&&size| size > charset.len()) {
        return Err(format!(
            "charset size {size} exceeds the {} distinct characters in --charset",
            charset.len()
        )
        .into());
    }
    let mut combos = Vec::new();
    for &length in &lengths {
        for &charset_size in &charset_sizes {
            for &offspring in &args.offspring.0 {
                for &mutation_rate in &args.mutation_rate.0 {
                    combos.push(Combo { length, charset_size, offspring, mutation_rate });
                }
            }
        }
    }

    // Run i of every combination gets its own RNG seeded with base_seed + i, so results don't
    // depend on thread count, and combinations with the same length and charset face the same
    // random targets.
    let make_run = |combo: Combo, i: u64| {
        let seed = base_seed.wrapping_add(i);
        let mut rng = StdRng::seed_from_u64(seed);
        let charset = charset[..combo.charset_size].to_vec();
        let target = match combo.length {
            Some(len) => random_string(&charset, len, &mut rng),
            None => args.target.chars().collect(),
        };
        let config = Config {
            target,
            charset,
            offspring: combo.offspring,
            mutation_rate: combo.mutation_rate,
            max_generations: args.max_generations,
        };
        (seed, rng, config)
    };
    // Within a combination, runs differ only in seed (and random target contents),
    // so checking one run per combination covers them all.
    for &combo in &combos {
        let (_, _, config) = make_run(combo, 0);
        config.validate().map_err(|e| {
            format!(
                "{e} (length {}, charset size {}, offspring {}, mutation rate {})",
                config.target.len(),
                combo.charset_size,
                combo.offspring,
                combo.mutation_rate
            )
        })?;
    }
    let total_runs = args
        .runs
        .checked_mul(combos.len() as u64)
        .ok_or("too many runs")?;
    if combos.len() > 1 && args.max_generations.is_none() {
        eprintln!(
            "warning: sweeping without --max-generations; some combinations (e.g. low offspring \
             with high mutation rates) may never reach the target"
        );
    }

    let batch_id = db
        .as_ref()
        .map(|db| {
            db.begin_batch(&BatchInfo {
                label: args.label.as_deref(),
                command: &command_line(),
                seed: base_seed,
                runs_per_combo: args.runs,
                combos: combos.len(),
                started_at_ms: now_ms(),
            })
        })
        .transpose()?;
    if let Some(id) = batch_id {
        eprintln!(
            "batch {id}: {} combination(s) x {} run(s) = {total_runs} runs",
            combos.len(),
            args.runs
        );
    }

    let threads = args
        .threads
        .or_else(|| thread::available_parallelism().ok())
        .map_or(1, NonZeroUsize::get)
        .min(usize::try_from(total_runs).unwrap_or(usize::MAX))
        .max(1);
    // Per-generation output from several threads would be interleaved.
    let verbose = !args.quiet && threads == 1;

    let next_run = AtomicU64::new(0);
    let (tx, rx) = mpsc::sync_channel::<RunRecord>(DB_BATCH_SIZE);

    let db = thread::scope(|s| {
        for _ in 0..threads {
            let tx = tx.clone();
            let (next_run, make_run, combos) = (&next_run, &make_run, &combos);
            s.spawn(move || {
                loop {
                    let job = next_run.fetch_add(1, Ordering::Relaxed);
                    if job >= total_runs {
                        break;
                    }
                    // Cycle through the combinations so partial results cover all of them.
                    let n = combos.len() as u64;
                    let combo = combos[(job % n) as usize];
                    let record = execute_run(make_run(combo, job / n), verbose);
                    // The receiver is gone only if writing to the database failed.
                    if tx.send(record).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        store_results(rx, db.zip(batch_id))
    })?;

    if let Some((db, id)) = db {
        db.finish_batch(id, now_ms())?;
    }
    Ok(())
}

fn execute_run((seed, mut rng, config): (u64, StdRng, Config), verbose: bool) -> RunRecord {
    let width = config.target.len();
    if verbose {
        let target: String = config.target.iter().collect();
        println!("seed:          {seed}");
        println!("target:        {target:?} (length {width})");
        println!("charset size:  {}", config.charset.len());
        println!("offspring:     {}", config.offspring);
        println!("mutation rate: {}", config.mutation_rate);
        println!();
    }

    let started_at_ms = now_ms();
    let start = Instant::now();
    let result = run(&config, &mut rng, |g| {
        if verbose {
            let s: String = g.best.iter().collect();
            println!("{:>6}  {s:?}  {:>3}/{width}", g.number, g.fitness);
        }
    })
    .expect("config was validated");
    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

    let label = format!(
        "seed {seed}, L={width}, k={}, n={}, m={}",
        config.charset.len(),
        config.offspring,
        config.mutation_rate
    );
    let summary = if result.reached_target {
        format!("{label}: reached target in {} generations", result.generations)
    } else {
        let best: String = result.best.iter().collect();
        format!(
            "{label}: stopped after {} generations; best {best:?} ({}/{width})",
            result.generations, result.fitness
        )
    };
    if verbose {
        println!("\n{summary}\n");
    } else {
        println!("{summary}");
    }

    RunRecord { config, seed, started_at_ms, duration_ms, result }
}

/// Writes finished runs to the database (if any) in batches as they arrive,
/// handing the database back once all workers are done.
fn store_results(
    rx: Receiver<RunRecord>,
    mut db: Option<(Database, i64)>,
) -> Result<Option<(Database, i64)>, Box<dyn std::error::Error>> {
    let mut pending = Vec::with_capacity(DB_BATCH_SIZE);
    while let Ok(record) = rx.recv() {
        let Some((db, batch_id)) = db.as_mut() else { continue };
        pending.push(record);
        pending.extend(rx.try_iter().take(DB_BATCH_SIZE - 1));
        db.record_runs(*batch_id, &pending)?;
        pending.clear();
    }
    Ok(db)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The command line as typed, with arguments containing whitespace quoted.
fn command_line() -> String {
    std::env::args()
        .map(|arg| {
            if arg.is_empty() || arg.contains(char::is_whitespace) {
                format!("{arg:?}")
            } else {
                arg
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
