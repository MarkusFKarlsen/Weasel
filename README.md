# weasel

A fast Rust implementation of Richard Dawkins' "weasel program", built for running large
parameter sweeps and analysing the results. Every run's settings and outcome can be stored in a
SQLite database, ready to query directly or to explore in Grafana.

## Contents

- [The algorithm](#the-algorithm)
- [Building](#building)
- [Quick start](#quick-start)
- [Options](#options)
- [Parameter sweeps](#parameter-sweeps)
- [Reproducibility and seeds](#reproducibility-and-seeds)
- [Parallelism and output](#parallelism-and-output)
- [Storing results in SQLite](#storing-results-in-sqlite)
- [Analysing results](#analysing-results)
- [Using Grafana](#using-grafana)
- [Errors and warnings](#errors-and-warnings)
- [Examples](#examples)

## The algorithm

Dawkins used the weasel program to show how **cumulative selection** gets to a target far faster
than pure chance could:

1. Start from a random string with the same length as the target.
2. Each generation, make `offspring` copies of the current parent.
3. Each character of each copy is replaced with a random character from the charset with
   probability `mutation-rate`. The replacement can be the same character it replaces.
4. The **fittest** copy (the one with the most characters matching the target in the same
   position) becomes the next parent. If several copies are equally fit, the first one wins.
5. Repeat until the parent matches the target, or until `--max-generations` is reached.

As in Dawkins' original, **the parent does not compete** with its offspring. So the best fitness
can drop from one generation to the next. Each drop is counted as a *regression*.

Defaults: target `methinks it is like a weasel`, charset `a`–`z` plus a space (27 characters),
100 offspring, mutation rate 0.05.

## Building

You need a Rust toolchain that supports edition 2024 (Rust 1.85 or newer). SQLite is bundled, so
nothing else needs to be installed.

```sh
cargo build --release
```

The binary is `target/release/weasel`. You can also run it through Cargo, putting the program's
own options after `--`:

```sh
cargo run --release -- --help
```

The examples below use `weasel` for the binary. Always use a release build for real
experiments, because debug builds are many times slower.

## Quick start

Run the classic program once and watch it evolve:

```sh
weasel --seed 42
```

```
seed:          42
target:        "methinks it is like a weasel" (length 28)
charset size:  27
offspring:     100
mutation rate: 0.05

     0  "dogoxr k aqljtfwqdmawzqnxktd"    1/28
     1  "rogoxr c aqljt wqdmmwzqnxkts"    2/28
     2  "rotoxr c aqljt wqdmmwzqnxkts"    3/28
     3  "rotoxr c aqljt lqdmmwzlkxnts"    4/28
...

seed 42, L=28, k=27, n=100, m=0.05: reached target in 113 generations
```

Each line shows the generation number, the best string of that generation, and its fitness.

(Per-generation output is only shown when the run uses a single thread. With a single run that
is automatic.)

Compare two offspring counts over 200 runs each and save the results:

```sh
weasel --db weasel.db --label "offspring 50 vs 100" -n 50,100 -r 200 --max-generations 20000 -q
```

## Options

Options marked **sweepable** accept several values. See [Parameter sweeps](#parameter-sweeps).

| Option | Short | Default | Description |
| --- | --- | --- | --- |
| `--target <TEXT>` | `-t` | `methinks it is like a weasel` | Target string to evolve towards. Every character must be in the charset. Can't be combined with `--length`. |
| `--length <L>` | `-l` | (not set) | Use a random target of length L instead of `--target`. **Sweepable.** |
| `--charset <CHARS>` | `-c` | `abcdefghijklmnopqrstuvwxyz ` | Characters used for mutations and random targets. Duplicate characters are ignored. |
| `--charset-size <K>` | `-k` | all characters | Use only the first K characters of the charset. **Sweepable.** |
| `--offspring <N>` | `-n` | `100` | Mutated copies made per generation. Must be at least 1. **Sweepable.** |
| `--mutation-rate <M>` | `-m` | `0.05` | Chance that each character is mutated, greater than 0 and at most 1. **Sweepable.** |
| `--max-generations <G>` | | unlimited | Stop a run after G generations, even if it hasn't reached the target. |
| `--seed <S>` | `-s` | random | Base seed for the random number generator. |
| `--runs <R>` | `-r` | `1` | Independent runs for every combination of swept values. |
| `--threads <J>` | `-j` | number of CPUs | Worker threads. |
| `--db <PATH>` | | (not set) | SQLite database to add results to. It is created if it doesn't exist. |
| `--label <TEXT>` | | (not set) | A description stored with this batch. Requires `--db`. |
| `--quiet` | `-q` | off | Print only one summary line per run. |
| `--help` | `-h` | | Show help. |
| `--version` | `-V` | | Show the version. |

### Target: `--target` or `--length`

By default, every run evolves towards the same fixed `--target`:

```sh
weasel -t "to be or not to be"
```

With `--length`, each run gets a **random** target of that length, drawn from the (possibly
reduced) charset. Use this to study how target length affects the time to reach the target,
without the results depending on one particular sentence:

```sh
weasel -l 20 -r 100 -q
```

### Charset: `--charset` and `--charset-size`

`--charset` sets the alphabet. `--charset-size K` uses only its first K characters, so the order
of the charset matters. With the default charset, `-k 2` means `ab`, and `-k 10` means
`abcdefghij`.

```sh
# Binary strings
weasel -c 01 -l 32

# DNA
weasel -c ACGT -t GATTACAGATTACA

# The effect of alphabet size on random targets of length 20
weasel -l 20 -k 2,5,10,27 -r 100 -q --max-generations 20000
```

A fixed `--target` must only use characters that remain after `--charset-size`. For example,
`-k 10` with the default target fails, because `m` isn't among the first 10 letters. Charset size
sweeps are therefore usually combined with `--length`.

### Offspring and mutation rate: `-n` and `-m`

These are the main selection parameters. More offspring per generation means fewer generations
(but more work per generation). The mutation rate has a sweet spot. If it is too low, progress is
slow. If it is too high, good characters are destroyed faster than selection can keep them.

```sh
weasel -n 500 -m 0.02
```

### Stopping: `--max-generations`

Some settings never reach the target. For example, with very few offspring and a high mutation
rate, fitness settles below the target length and stays there. Set a cap whenever you sweep:

```sh
weasel -n 5 -m 0.1 --max-generations 300 -q
```

```
seed 1, L=28, k=27, n=5, m=0.1: stopped after 300 generations; best "detkrsbtwitssselidejcnweasui" (12/28)
```

## Parameter sweeps

The sweepable options (`-l`, `-k`, `-n`, `-m`) each accept:

- a single value: `-n 100`
- a comma-separated list: `-n 10,50,100`
- an inclusive range `start:end:step`: `-n 10:100:10` (10, 20, …, 100)
- any mix of these: `-n 1:5:1,10,50:200:50` (1, 2, 3, 4, 5, 10, 50, 100, 150, 200)

The end of a range is included when the steps land on it exactly. `10:55:20` gives 10, 30, 50.
Decimal ranges are rounded to clean values, so `-m 0.01:0.1:0.01` gives 0.01, 0.02, …, 0.1, and
not values like 0.030000000000000002. This means the values group correctly in SQL queries.

When several options are swept, **every combination** is run (a full grid), and each combination
is run `--runs` times:

```sh
# 3 lengths x 2 charset sizes x 4 offspring counts = 24 combinations; x 50 runs = 1200 runs
weasel -l 10,20,30 -k 2,27 -n 25,50,100,200 -r 50 --max-generations 20000 -q
```

Runs are started in rotation through the combinations (first run of each combination, then the
second run of each, and so on). If you stop a large batch early with Ctrl+C, the results you have
cover every combination about equally.

## Reproducibility and seeds

Every run can be recreated exactly:

- Without `--seed`, a random base seed is chosen. It is printed in every summary line and stored
  in the database.
- Run *i* (counting from 0) of **every** combination uses seed `base seed + i`.
- The results don't depend on the number of threads. `-j 1` and `-j 32` produce the same runs.

A useful result: combinations that share a target length, charset size and seed also get the
**same random target**. When you sweep `-n` or `-m`, each run number is compared on the same
target, which makes the comparison fairer.

To replay one run from a batch with its per-generation output, use its seed and settings with
`--runs 1`:

```sh
weasel -s 43 -n 50
```

For a random target, also give the same `--length`, `--charset` and `--charset-size`.

## Parallelism and output

Runs execute in parallel on all CPU cores by default. Use `-j` to limit this:

```sh
weasel -r 1000 -q -j 8
```

What is printed depends on the thread count:

- **One thread** (`-j 1`, or a single run) without `-q`: the settings, every generation, and a
  summary.
- **Several threads**, or `-q`: one summary line per run, in the order they finish:

```
seed 44, L=28, k=27, n=50, m=0.05: reached target in 44 generations
seed 43, L=28, k=27, n=100, m=0.05: reached target in 80 generations
```

In the summary, `L` is the target length, `k` the charset size, `n` the offspring count and `m` the
mutation rate.

Messages about the batch, warnings and errors go to stderr, so you can redirect run output
separately:

```sh
weasel -r 10000 -q > runs.log
```

## Storing results in SQLite

Add `--db <path>` to store results. The file is created if it doesn't exist. Later
invocations add to it, and each invocation is recorded as a **batch**:

```sh
weasel --db weasel.db --label "mutation rate sweep" -m 0.01:0.2:0.01 -r 100 --max-generations 20000 -q
```

```
batch 1: 20 combination(s) x 100 run(s) = 2000 runs
```

Only final results are stored, one row per run. Per-generation history is not stored.

The database uses WAL mode, so you can query it (or view it in Grafana) while a batch is still
running.

If a database was created by an older version of weasel with a different layout, it is rejected
with `database has an outdated runs table`. Use a new database file in that case.

### Table `batches` (one row per invocation)

| Column | Description |
| --- | --- |
| `id` | Batch id, referenced by `runs.batch_id` |
| `label` | The `--label` text, or NULL |
| `command` | The full command line |
| `seed` | Base seed (text, because the value may be too large for a SQLite integer) |
| `runs_per_combo` | `--runs` |
| `combos` | Number of parameter combinations |
| `started_at` | Start time, Unix epoch in **milliseconds** |
| `finished_at` | End time, in milliseconds. **NULL if the batch was interrupted or failed.** |

### Table `runs` (one row per run)

| Column | Description |
| --- | --- |
| `id` | Run id |
| `batch_id` | The batch this run belongs to |
| `started_at` | Start time, Unix epoch in milliseconds |
| `duration_ms` | Time the run took, in milliseconds |
| `seed` | This run's seed (text) |
| `target` | Target string |
| `target_length` | Length of the target (L) |
| `charset` | The characters actually used (after `--charset-size`) |
| `charset_size` | Number of characters in the charset (k) |
| `offspring` | Offspring per generation (n) |
| `mutation_rate` | Mutation rate (m) |
| `max_generations` | The generation cap, or NULL if there was none |
| `generations` | Generations until the target was reached or the run stopped |
| `reached_target` | 1 if the target was reached, 0 if the cap was hit |
| `initial_fitness` | Fitness of the random starting string |
| `final_fitness` | Fitness when the run ended |
| `peak_fitness` | Highest fitness reached during the run |
| `regressions` | Number of generations in which fitness dropped |

## Analysing results

You can query the database with any SQLite client, for example the `sqlite3` command-line tool.

List the batches and whether they finished:

```sql
SELECT id, label, runs_per_combo * combos AS planned_runs,
       (SELECT COUNT(*) FROM runs WHERE batch_id = batches.id) AS stored_runs,
       finished_at IS NOT NULL AS finished
FROM batches;
```

Average generations and success rate per offspring count and mutation rate in batch 1:

```sql
SELECT offspring, mutation_rate,
       COUNT(*)             AS runs,
       AVG(reached_target)  AS success_rate,
       AVG(generations)     AS avg_generations
FROM runs
WHERE batch_id = 1
GROUP BY offspring, mutation_rate
ORDER BY offspring, mutation_rate;
```

Generations against target length and charset size, only for combinations where every run
reached the target:

```sql
SELECT target_length, charset_size, AVG(generations) AS avg_generations
FROM runs
WHERE batch_id = 2
GROUP BY target_length, charset_size
HAVING MIN(reached_target) = 1
ORDER BY target_length, charset_size;
```

Things to keep in mind:

- **Runs that hit the cap distort averages.** Their `generations` equals `--max-generations`,
  not the true number, so `AVG(generations)` is too low for combinations where some runs failed.
  Always look at `AVG(reached_target)` too, or only compare combinations where every run
  reached the target (as in the query above).
- **Timings are only comparable from single-threaded runs.** When runs share the CPU,
  `duration_ms` is higher. Use `-j 1` for timing experiments.
- **`regressions` is almost always 0 with many offspring.** With 100 copies, at least one
  usually keeps the parent's fitness. It becomes interesting at low `-n`.
- The `seed` column is text. Compare it with a string (`WHERE seed = '43'`) or use
  `CAST(seed AS INTEGER)` for small seeds.

## Using Grafana

Grafana can read the database with the
[SQLite data source plugin](https://grafana.com/grafana/plugins/frser-sqlite-datasource/)
(`frser-sqlite-datasource`). Point the data source at the database file, then use queries like
the ones above.

Timestamps are stored in **milliseconds**. For a time series, convert them to seconds:

```sql
SELECT started_at / 1000 AS time, generations
FROM runs
WHERE batch_id = 3
ORDER BY started_at;
```

For parameter comparisons (for example generations against mutation rate), use a table or an
XY chart panel with a `GROUP BY` query rather than a time series.

## Errors and warnings

All combinations are checked before anything runs. Errors name the combination that failed:

```
error: target contains 'm', which is not in the charset (length 28, charset size 10, offspring 100, mutation rate 0.05)
error: mutation rate must be in (0, 1], got 0 (length 28, charset size 27, offspring 100, mutation rate 0)
error: offspring per generation must be at least 1 (length 28, charset size 27, offspring 0, mutation rate 0.05)
error: charset size 30 exceeds the 27 distinct characters in --charset
```

Invalid sweep syntax is reported by the argument parser, for example for a range with a step of
0, a start greater than its end, or more than 100,000 values in a single sweep.

Sweeping without `--max-generations` prints a warning, because some combinations may never
finish:

```
warning: sweeping without --max-generations; some combinations (e.g. low offspring with high mutation rates) may never reach the target
```

For example, with `-n 10` and a mutation rate of 0.07 or higher, the default target is never
reached, and with `-n 5 -m 0.1`, fitness stays around 16 out of 28.

## Examples

**The classic program, with an uppercase alphabet:**

```sh
weasel -c "ABCDEFGHIJKLMNOPQRSTUVWXYZ " -t "METHINKS IT IS LIKE A WEASEL"
```

**How consistent is the classic setup? 10,000 runs:**

```sh
weasel --db weasel.db --label "classic x10000" -r 10000 -q
```

**Find the best mutation rate for several offspring counts:**

```sh
weasel --db weasel.db --label "n vs m" \
  -n 10,25,50,100,250 -m 0.005:0.15:0.005 -r 200 --max-generations 20000 -q
```

**How target length and alphabet size affect the number of generations:**

```sh
weasel --db weasel.db --label "L vs k" -l 5:30:5 -k 2,10,27 -r 200 --max-generations 20000 -q
```

**When does selection fail? Low offspring and high mutation rates, with regressions:**

```sh
weasel --db weasel.db --label "breakdown" -n 2:20:2 -m 0.02:0.2:0.02 -r 100 --max-generations 5000 -q
```

**Timing measurements (single thread, so durations are comparable):**

```sh
weasel --db weasel.db --label "timing" -n 50,100,200,400 -r 50 -j 1 -q
```

**A reproducible batch that someone else can recreate:**

```sh
weasel --db weasel.db --label "shared experiment" -s 12345 -n 50,100 -r 500 --max-generations 20000 -q
```

The same command with the same seed gives identical results on any machine and with any number
of threads.
