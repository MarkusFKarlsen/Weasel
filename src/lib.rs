//! Dawkins' weasel program: cumulative selection towards a target string.
//!
//! Each generation, the current parent produces `offspring` copies. Every
//! character of every copy is independently replaced with a random character
//! from the charset with probability `mutation_rate`. The copy that matches the
//! target in the most positions becomes the next parent.

pub mod db;

use rand::seq::IndexedRandom;
use rand::{Rng, RngExt};
use std::fmt;

pub const DEFAULT_TARGET: &str = "methinks it is like a weasel";
pub const DEFAULT_CHARSET: &str = "abcdefghijklmnopqrstuvwxyz ";
pub const DEFAULT_OFFSPRING: usize = 100;
pub const DEFAULT_MUTATION_RATE: f64 = 0.05;

#[derive(Debug, Clone)]
pub struct Config {
    pub target: Vec<char>,
    pub charset: Vec<char>,
    /// Number of mutated copies produced per generation.
    pub offspring: usize,
    /// Per-character probability of being replaced by a random charset character.
    pub mutation_rate: f64,
    /// Stop after this many generations even if the target isn't reached.
    pub max_generations: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            target: DEFAULT_TARGET.chars().collect(),
            charset: DEFAULT_CHARSET.chars().collect(),
            offspring: DEFAULT_OFFSPRING,
            mutation_rate: DEFAULT_MUTATION_RATE,
            max_generations: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    EmptyTarget,
    EmptyCharset,
    NoOffspring,
    InvalidMutationRate(f64),
    CharNotInCharset(char),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTarget => write!(f, "target string must not be empty"),
            Self::EmptyCharset => write!(f, "charset must not be empty"),
            Self::NoOffspring => write!(f, "offspring per generation must be at least 1"),
            Self::InvalidMutationRate(r) => {
                write!(f, "mutation rate must be in (0, 1], got {r}")
            }
            Self::CharNotInCharset(c) => {
                write!(f, "target contains {c:?}, which is not in the charset")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.target.is_empty() {
            return Err(ConfigError::EmptyTarget);
        }
        if self.charset.is_empty() {
            return Err(ConfigError::EmptyCharset);
        }
        if self.offspring == 0 {
            return Err(ConfigError::NoOffspring);
        }
        if !(self.mutation_rate > 0.0 && self.mutation_rate <= 1.0) {
            return Err(ConfigError::InvalidMutationRate(self.mutation_rate));
        }
        if let Some(&c) = self.target.iter().find(|c| !self.charset.contains(c)) {
            return Err(ConfigError::CharNotInCharset(c));
        }
        Ok(())
    }
}

/// Removes duplicate characters while preserving first-occurrence order.
pub fn dedup_charset(chars: impl IntoIterator<Item = char>) -> Vec<char> {
    let mut out = Vec::new();
    for c in chars {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

pub fn random_string<R: Rng + ?Sized>(charset: &[char], len: usize, rng: &mut R) -> Vec<char> {
    (0..len).map(|_| *charset.choose(rng).unwrap()).collect()
}

/// Number of positions where `candidate` matches `target`.
pub fn fitness(candidate: &[char], target: &[char]) -> usize {
    candidate.iter().zip(target).filter(|(a, b)| a == b).count()
}

pub fn mutate<R: Rng + ?Sized>(
    parent: &[char],
    charset: &[char],
    rate: f64,
    rng: &mut R,
) -> Vec<char> {
    parent
        .iter()
        .map(|&c| {
            if rng.random_bool(rate) {
                *charset.choose(rng).unwrap()
            } else {
                c
            }
        })
        .collect()
}

/// State of the population after a generation.
#[derive(Debug, Clone)]
pub struct Generation<'a> {
    /// 0 is the initial random string.
    pub number: u64,
    pub best: &'a [char],
    pub fitness: usize,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub generations: u64,
    pub reached_target: bool,
    pub best: Vec<char>,
    pub fitness: usize,
    /// Fitness of the initial random string.
    pub initial_fitness: usize,
    /// Generations whose best offspring was less fit than its parent.
    pub regressions: u64,
    /// Highest fitness seen in any generation.
    pub peak_fitness: usize,
}

/// Runs the simulation, calling `on_generation` for the initial string and after every generation.
pub fn run<R: Rng + ?Sized>(
    config: &Config,
    rng: &mut R,
    mut on_generation: impl FnMut(&Generation),
) -> Result<RunResult, ConfigError> {
    config.validate()?;
    let target = &config.target;
    let len = target.len();

    let mut parent = random_string(&config.charset, len, rng);
    let mut parent_fitness = fitness(&parent, target);
    let initial_fitness = parent_fitness;
    let mut peak_fitness = parent_fitness;
    let mut regressions = 0;
    let mut number = 0;
    on_generation(&Generation { number, best: &parent, fitness: parent_fitness });

    while parent_fitness < len && config.max_generations.is_none_or(|max| number < max) {
        number += 1;
        let (child, child_fitness) = (0..config.offspring)
            .map(|_| {
                let child = mutate(&parent, &config.charset, config.mutation_rate, rng);
                let f = fitness(&child, target);
                (child, f)
            })
            // First child wins ties.
            .reduce(|best, cand| if cand.1 > best.1 { cand } else { best })
            .unwrap();
        if child_fitness < parent_fitness {
            regressions += 1;
        }
        peak_fitness = peak_fitness.max(child_fitness);
        parent = child;
        parent_fitness = child_fitness;
        on_generation(&Generation { number, best: &parent, fitness: parent_fitness });
    }

    Ok(RunResult {
        generations: number,
        reached_target: parent_fitness == len,
        best: parent,
        fitness: parent_fitness,
        initial_fitness,
        regressions,
        peak_fitness,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn fitness_counts_matching_positions() {
        let t: Vec<char> = "abc d".chars().collect();
        assert_eq!(fitness(&"abc d".chars().collect::<Vec<_>>(), &t), 5);
        assert_eq!(fitness(&"xbcxd".chars().collect::<Vec<_>>(), &t), 3);
    }

    #[test]
    fn default_config_is_valid() {
        assert_eq!(Config::default().validate(), Ok(()));
    }

    #[test]
    fn rejects_target_chars_outside_charset() {
        let config = Config { target: "Hello".chars().collect(), ..Config::default() };
        assert_eq!(config.validate(), Err(ConfigError::CharNotInCharset('H')));
    }

    #[test]
    fn rejects_bad_mutation_rate() {
        for rate in [0.0, -0.1, 1.5, f64::NAN] {
            let config = Config { mutation_rate: rate, ..Config::default() };
            assert!(config.validate().is_err(), "rate {rate} accepted");
        }
    }

    #[test]
    fn dedup_preserves_order() {
        assert_eq!(dedup_charset("abca b".chars()), vec!['a', 'b', 'c', ' ']);
    }

    #[test]
    fn default_run_reaches_target() {
        let mut rng = StdRng::seed_from_u64(1);
        let result = run(&Config::default(), &mut rng, |_| {}).unwrap();
        assert!(result.reached_target);
        assert_eq!(result.best, Config::default().target);
    }

    #[test]
    fn same_seed_gives_same_run() {
        let gens = |seed| {
            let mut rng = StdRng::seed_from_u64(seed);
            run(&Config::default(), &mut rng, |_| {}).unwrap().generations
        };
        assert_eq!(gens(42), gens(42));
    }

    #[test]
    fn max_generations_stops_run() {
        let config = Config { offspring: 1, max_generations: Some(3), ..Config::default() };
        let mut rng = StdRng::seed_from_u64(0);
        let mut seen = 0;
        let result = run(&config, &mut rng, |_| seen += 1).unwrap();
        assert_eq!(result.generations, 3);
        assert!(!result.reached_target);
        assert_eq!(seen, 4);
    }
}
