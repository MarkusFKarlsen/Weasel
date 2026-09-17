//! Parameter sweeps given on the command line, e.g. `0.01,0.05` or `10:100:10`.
//!
//! A sweep is a comma-separated list of items, where each item is either a single
//! value or an inclusive range `start:end:step`.

use std::fmt;
use std::str::FromStr;

/// Upper bound on values in a single sweep, to catch typos like a tiny step.
const MAX_VALUES: usize = 100_000;

pub trait SweepValue: Copy + PartialOrd + FromStr + fmt::Display {
    /// The `k`th value of a range starting at `start`.
    fn nth(start: Self, step: Self, k: usize) -> Self;
    fn is_positive(self) -> bool;
    /// Whether `value` is past `end`, allowing for rounding error.
    fn past_end(value: Self, end: Self, step: Self) -> bool;
}

impl SweepValue for f64 {
    fn nth(start: f64, step: f64, k: usize) -> f64 {
        // Computed from `start` rather than accumulated, and rounded, so that
        // e.g. 0.01:0.1:0.01 yields 0.03 rather than 0.030000000000000002.
        let v = start + step * k as f64;
        (v * 1e9).round() / 1e9
    }
    fn is_positive(self) -> bool {
        self > 0.0
    }
    fn past_end(value: f64, end: f64, step: f64) -> bool {
        value > end + step * 1e-9
    }
}

impl SweepValue for usize {
    fn nth(start: usize, step: usize, k: usize) -> usize {
        start + step * k
    }
    fn is_positive(self) -> bool {
        self > 0
    }
    fn past_end(value: usize, end: usize, _step: usize) -> bool {
        value > end
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sweep<T>(pub Vec<T>);

impl<T: SweepValue> FromStr for Sweep<T> {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let parse = |v: &str| {
            v.trim().parse::<T>().map_err(|_| format!("invalid value {:?}", v.trim()))
        };
        let mut values = Vec::new();
        for item in s.split(',') {
            match item.split(':').collect::<Vec<_>>()[..] {
                [v] => values.push(parse(v)?),
                [start, end, step] => {
                    let (start, end, step) = (parse(start)?, parse(end)?, parse(step)?);
                    if !step.is_positive() {
                        return Err(format!("step in {item:?} must be positive"));
                    }
                    if start > end {
                        return Err(format!("start is greater than end in {item:?}"));
                    }
                    for k in 0.. {
                        let v = T::nth(start, step, k);
                        if T::past_end(v, end, step) {
                            break;
                        }
                        if values.len() >= MAX_VALUES {
                            return Err(format!("sweep has more than {MAX_VALUES} values"));
                        }
                        values.push(v);
                    }
                }
                _ => return Err(format!("{item:?} is neither a value nor start:end:step")),
            }
        }
        Ok(Self(values))
    }
}

impl<T: fmt::Display> fmt::Display for Sweep<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, v) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{v}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse<T: SweepValue>(s: &str) -> Result<Vec<T>, String> {
        s.parse::<Sweep<T>>().map(|s| s.0)
    }

    #[test]
    fn single_value() {
        assert_eq!(parse::<f64>("0.05"), Ok(vec![0.05]));
        assert_eq!(parse::<usize>("100"), Ok(vec![100]));
    }

    #[test]
    fn list() {
        assert_eq!(parse::<usize>("10, 20,50"), Ok(vec![10, 20, 50]));
    }

    #[test]
    fn integer_range_is_inclusive() {
        assert_eq!(parse::<usize>("10:50:20"), Ok(vec![10, 30, 50]));
        assert_eq!(parse::<usize>("10:55:20"), Ok(vec![10, 30, 50]));
    }

    #[test]
    fn float_range_has_clean_values_and_includes_end() {
        assert_eq!(parse::<f64>("0.01:0.05:0.01"), Ok(vec![0.01, 0.02, 0.03, 0.04, 0.05]));
        assert_eq!(parse::<f64>("0.1:0.3:0.1"), Ok(vec![0.1, 0.2, 0.3]));
    }

    #[test]
    fn mixed_ranges_and_values() {
        assert_eq!(parse::<usize>("1:3:1,10,100:300:100"), Ok(vec![1, 2, 3, 10, 100, 200, 300]));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse::<usize>("").is_err());
        assert!(parse::<usize>("abc").is_err());
        assert!(parse::<usize>("1:10").is_err());
        assert!(parse::<usize>("10:1:1").is_err());
        assert!(parse::<usize>("1:10:0").is_err());
        assert!(parse::<f64>("0:1:0.000000001").is_err());
    }

    #[test]
    fn display_round_trips() {
        let s: Sweep<f64> = "0.01:0.03:0.01,0.5".parse().unwrap();
        assert_eq!(s.to_string(), "0.01,0.02,0.03,0.5");
        assert_eq!(s.to_string().parse::<Sweep<f64>>(), Ok(s));
    }
}
