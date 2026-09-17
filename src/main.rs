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
use weasel::db::{Database, RunRecord};
use weasel::{
    Config, DEFAULT_CHARSET, DEFAULT_MUTATION_RATE, DEFAULT_OFFSPRING, DEFAULT_TARGET,
    dedup_charset, random_string, run,
};

/// Maximum number of finished runs written to the database per transaction.
const DB_BATCH_SIZE: usize = 1000;

/// Dawkins' weasel program.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Target string to evolve towards
    #[arg(short, long, default_value = DEFAULT_TARGET, conflicts_with = "length")]
    target: String,

    /// Use a random target of this length (drawn from the charset) instead of --target
    #[arg(short, long)]
    length: Option<usize>,

    /// Valid characters for mutations (and the target)
    #[arg(short, long, default_value = DEFAULT_CHARSET)]
    charset: String,

    /// Mutated copies (offspring) produced per generation
    #[arg(short = 'n', long, default_value_t = DEFAULT_OFFSPRING)]
    offspring: usize,

    /// Per-character mutation probability, in (0, 1]
    #[arg(short, long, default_value_t = DEFAULT_MUTATION_RATE)]
    mutation_rate: f64,

    /// Give up after this many generations
    #[arg(long)]
    max_generations: Option<u64>,

    /// RNG seed for reproducible runs (random if omitted); run i uses seed + i
    #[arg(short, long)]
    seed: Option<u64>,

    /// Number of independent runs to perform
    #[arg(short, long, default_value_t = 1)]
    runs: u64,

    /// Worker threads for running in parallel [default: number of CPUs]
    #[arg(short = 'j', long)]
    threads: Option<NonZeroUsize>,

    /// SQLite database to append results to (created if missing)
    #[arg(long)]
    db: Option<PathBuf>,

    /// Only print one summary line per run (implied when using more than one thread)
    #[arg(short, long)]
    quiet: bool,
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

    // Run i gets its own RNG seeded with base_seed + i, so results don't depend on thread count.
    let make_run = |i: u64| {
        let seed = base_seed.wrapping_add(i);
        let mut rng = StdRng::seed_from_u64(seed);
        let target = match args.length {
            Some(len) => random_string(&charset, len, &mut rng),
            None => args.target.chars().collect(),
        };
        let config = Config {
            target,
            charset: charset.clone(),
            offspring: args.offspring,
            mutation_rate: args.mutation_rate,
            max_generations: args.max_generations,
        };
        (seed, rng, config)
    };
    // Runs differ only in seed (and random target contents), so one check covers them all.
    make_run(0).2.validate()?;

    let threads = args
        .threads
        .or_else(|| thread::available_parallelism().ok())
        .map_or(1, NonZeroUsize::get)
        .min(usize::try_from(args.runs).unwrap_or(usize::MAX))
        .max(1);
    // Per-generation output from several threads would be interleaved.
    let verbose = !args.quiet && threads == 1;

    let next_run = AtomicU64::new(0);
    let (tx, rx) = mpsc::sync_channel::<RunRecord>(DB_BATCH_SIZE);

    thread::scope(|s| {
        for _ in 0..threads {
            let tx = tx.clone();
            let (next_run, make_run) = (&next_run, &make_run);
            s.spawn(move || {
                loop {
                    let i = next_run.fetch_add(1, Ordering::Relaxed);
                    if i >= args.runs {
                        break;
                    }
                    let record = execute_run(make_run(i), verbose);
                    // The receiver is gone only if writing to the database failed.
                    if tx.send(record).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        store_results(rx, db)
    })
}

fn execute_run((seed, mut rng, config): (u64, StdRng, Config), verbose: bool) -> RunRecord {
    let width = config.target.len();
    if verbose {
        let target: String = config.target.iter().collect();
        println!("seed:          {seed}");
        println!("target:        {target:?} (length {width})");
        println!("offspring:     {}", config.offspring);
        println!("mutation rate: {}", config.mutation_rate);
        println!();
    }

    let started_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let start = Instant::now();
    let result = run(&config, &mut rng, |g| {
        if verbose {
            let s: String = g.best.iter().collect();
            println!("{:>6}  {s:?}  {:>3}/{width}", g.number, g.fitness);
        }
    })
    .expect("config was validated");
    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

    let summary = if result.reached_target {
        format!("seed {seed}: reached target in {} generations", result.generations)
    } else {
        let best: String = result.best.iter().collect();
        format!(
            "seed {seed}: stopped after {} generations; best {best:?} ({}/{width})",
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

/// Writes finished runs to the database (if any) in batches as they arrive.
fn store_results(
    rx: Receiver<RunRecord>,
    mut db: Option<Database>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut batch = Vec::with_capacity(DB_BATCH_SIZE);
    while let Ok(record) = rx.recv() {
        let Some(db) = db.as_mut() else { continue };
        batch.push(record);
        batch.extend(rx.try_iter().take(DB_BATCH_SIZE - 1));
        db.record_runs(&batch)?;
        batch.clear();
    }
    Ok(())
}
