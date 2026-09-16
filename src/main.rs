use clap::Parser;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use weasel::db::Database;
use weasel::{
    Config, DEFAULT_CHARSET, DEFAULT_MUTATION_RATE, DEFAULT_OFFSPRING, DEFAULT_TARGET,
    dedup_charset, random_string, run,
};

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

    /// SQLite database to append results to (created if missing)
    #[arg(long)]
    db: Option<PathBuf>,

    /// Only print one summary line per run
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

    for i in 0..args.runs {
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
        config.validate()?;

        let width = config.target.len();
        if !args.quiet {
            let target: String = config.target.iter().collect();
            println!("seed:          {seed}");
            println!("target:        {target:?} (length {width})");
            println!("offspring:     {}", config.offspring);
            println!("mutation rate: {}", config.mutation_rate);
            println!();
        }

        let started_at_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i64;
        let start = Instant::now();
        let result = run(&config, &mut rng, |g| {
            if !args.quiet {
                let s: String = g.best.iter().collect();
                println!("{:>6}  {s:?}  {:>3}/{width}", g.number, g.fitness);
            }
        })?;
        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

        let run_id = db
            .as_ref()
            .map(|db| db.record_run(&config, seed, started_at_ms, duration_ms, &result))
            .transpose()?;

        if !args.quiet {
            println!();
        }
        let id = run_id.map(|id| format!(" [run id {id}]")).unwrap_or_default();
        if result.reached_target {
            println!("seed {seed}: reached target in {} generations{id}", result.generations);
        } else {
            let best: String = result.best.iter().collect();
            println!(
                "seed {seed}: stopped after {} generations; best {best:?} ({}/{width}){id}",
                result.generations, result.fitness
            );
        }
        if !args.quiet && i + 1 < args.runs {
            println!();
        }
    }
    Ok(())
}
