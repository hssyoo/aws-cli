//! `partitions.json` → [`Partitions`], and `aws.partition`'s lookup, as
//! the CRT engine reads them (`partitions.c`, `endpoints_regex.c`): a
//! region's own entry first, then the first partition whose `regionRegex`
//! matches, then the partition `aws`.
use crate::error::{DecodeError, EvalError};
use crate::ir::EvalValue;
use serde_json::Value;

/// The partitions `aws.partition` matches a region against.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Partitions {
    /// Each listed region and its partition's outputs, merged with the
    /// region's overrides.
    regions: Vec<(Box<str>, EvalValue)>,
    partitions: Vec<Partition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Partition {
    id: Box<str>,
    regex: Option<Regex>,
    outputs: EvalValue,
}

type Result<T> = std::result::Result<T, DecodeError>;

fn fail<T>(detail: impl Into<String>) -> Result<T> {
    Err(DecodeError::partitions(detail.into()))
}

impl Partitions {
    /// No partitions: `aws.partition` then fails.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Decodes a `partitions.json`.
    pub fn decode(json: &[u8]) -> Result<Self> {
        let root: Value =
            serde_json::from_slice(json).or_else(|error| fail(format!("not JSON: {error}")))?;
        if !root.get("version").is_some_and(Value::is_string) {
            return fail("version is not a string");
        }
        let Some(list) = root.get("partitions").and_then(Value::as_array) else {
            return fail("partitions is not an array");
        };
        let mut out = Self::default();
        for partition in list {
            out.add(partition)?;
        }
        Ok(out)
    }

    fn add(&mut self, partition: &Value) -> Result<()> {
        let Some(id) = partition.get("id").and_then(Value::as_str) else {
            return fail("a partition's id is not a string");
        };
        let Some(outputs) = partition.get("outputs").and_then(Value::as_object) else {
            return fail(format!("partition {id}: outputs is not an object"));
        };
        let regex = match partition.get("regionRegex") {
            None => None,
            Some(Value::String(pattern)) => Some(
                Regex::new(pattern)
                    .ok_or(())
                    .or_else(|()| fail(format!("partition {id}: unsupported regex {pattern:?}")))?,
            ),
            Some(_) => return fail(format!("partition {id}: regionRegex is not a string")),
        };
        let fields = outputs
            .iter()
            .map(|(k, v)| Ok((k.as_str().into(), value(v)?)));
        let fields: Vec<(Box<str>, EvalValue)> = fields.collect::<Result<_>>()?;
        let regions = partition
            .get("regions")
            .and_then(Value::as_object)
            .into_iter()
            .flatten();
        for (region, overrides) in regions {
            let Some(overrides) = overrides.as_object() else {
                return fail(format!("region {region} is not an object"));
            };
            let mut merged = fields.clone();
            // As the CRT engine does, a region's description is not an output.
            for (key, v) in overrides.iter().filter(|(key, _)| *key != "description") {
                merged.retain(|(name, _)| **name != **key);
                merged.push((key.as_str().into(), value(v)?));
            }
            self.regions.retain(|(name, _)| **name != **region);
            self.regions
                .push((region.as_str().into(), EvalValue::Record(merged.into())));
        }
        // A later partition of the same id replaces an earlier one.
        self.partitions.retain(|p| *p.id != *id);
        self.partitions.push(Partition {
            id: id.into(),
            regex,
            outputs: EvalValue::Record(fields.into()),
        });
        Ok(())
    }

    /// `aws.partition(region)`.
    pub(crate) fn lookup(&self, region: &str) -> std::result::Result<EvalValue, EvalError> {
        if let Some((_, outputs)) = self.regions.iter().find(|(name, _)| **name == *region) {
            return Ok(outputs.clone());
        }
        let matched = self
            .partitions
            .iter()
            .find(|p| p.regex.as_ref().is_some_and(|r| r.matches(region)));
        let partition = matched.or_else(|| self.partitions.iter().find(|p| &*p.id == "aws"));
        partition.map(|p| p.outputs.clone()).ok_or_else(|| {
            EvalError::new(format!("no partition for {region:?} and no aws partition"))
        })
    }
}

/// A partition output as a value: strings, booleans, integers, arrays and
/// objects.
fn value(json: &Value) -> Result<EvalValue> {
    Ok(match json {
        Value::String(text) => EvalValue::str(text),
        Value::Bool(flag) => EvalValue::Bool(*flag),
        Value::Number(n) => EvalValue::Int(
            n.as_i64()
                .map_or_else(|| fail(format!("{n} is not an integer")), Ok)?,
        ),
        Value::Array(items) => {
            EvalValue::Array(items.iter().map(value).collect::<Result<Vec<_>>>()?.into())
        }
        Value::Object(fields) => EvalValue::Record(
            fields
                .iter()
                .map(|(k, v)| Ok((k.as_str().into(), value(v)?)))
                .collect::<Result<Vec<_>>>()?
                .into(),
        ),
        Value::Null => return fail("null is not an output"),
    })
}

/// The CRT engine's minimal regex: `^…$` over letters and digits,
/// `.`, `\d`, `\w` (a letter), escaped characters, `*` and `+` (greedy,
/// never giving back, and followed by `-` unless last) and alternation
/// groups `(a|b)` (no alternative a prefix of another; the first that
/// prefixes the text wins). A match need not reach the end of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Regex(Vec<Symbol>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Symbol {
    Dot,
    Star,
    Plus,
    Digit,
    Alpha,
    Char(u8),
    Alternation(Vec<Box<[u8]>>),
}

impl Regex {
    fn new(pattern: &str) -> Option<Self> {
        if pattern.is_empty() || pattern.len() > 60 {
            return None;
        }
        let body = pattern.strip_prefix('^')?.strip_suffix('$')?.as_bytes();
        let mut symbols = Vec::new();
        let mut at = 0;
        while let Some(&byte) = body.get(at) {
            at += 1;
            symbols.push(match byte {
                b'.' => Symbol::Dot,
                b'*' => Symbol::Star,
                b'+' => Symbol::Plus,
                b'\\' => {
                    at += 1;
                    match *body.get(at - 1)? {
                        b'd' => Symbol::Digit,
                        b'w' => Symbol::Alpha,
                        other => Symbol::Char(other),
                    }
                }
                b'(' => {
                    let len = body[at..].iter().position(|&b| b == b')')?;
                    let group = &body[at..at + len];
                    at += len + 1;
                    Symbol::Alternation(group.split(|&b| b == b'|').map(Into::into).collect())
                }
                other if other.is_ascii_alphanumeric() => Symbol::Char(other),
                _ => return None,
            });
        }
        symbols
            .iter()
            .enumerate()
            .all(|(i, s)| valid(&symbols, i, s))
            .then_some(Self(symbols))
    }

    fn matches(&self, region: &str) -> bool {
        let mut text = region.as_bytes();
        if text.is_empty() || text.len() > 50 {
            return false;
        }
        let mut i = 0;
        while let Some(symbol) = self.0.get(i) {
            let modifier = self
                .0
                .get(i + 1)
                .filter(|s| matches!(s, Symbol::Star | Symbol::Plus));
            let ok = match (modifier, symbol) {
                (Some(modifier), _) => {
                    let run = text.iter().take_while(|&&b| one(symbol, b)).count();
                    text = &text[run..];
                    i += 1;
                    run > 0 || *modifier == Symbol::Star
                }
                (None, Symbol::Alternation(variants)) => {
                    match variants.iter().find(|v| text.starts_with(v)) {
                        Some(variant) => {
                            text = &text[variant.len()..];
                            true
                        }
                        None => false,
                    }
                }
                (None, _) => match text.split_first() {
                    Some((&byte, rest)) if one(symbol, byte) => {
                        text = rest;
                        true
                    }
                    _ => false,
                },
            };
            if !ok {
                return false;
            }
            i += 1;
        }
        true
    }
}

fn one(symbol: &Symbol, byte: u8) -> bool {
    match symbol {
        Symbol::Alpha => byte.is_ascii_alphabetic(),
        Symbol::Digit => byte.is_ascii_digit(),
        Symbol::Char(c) => byte == *c,
        Symbol::Dot => true,
        Symbol::Star | Symbol::Plus | Symbol::Alternation(_) => false,
    }
}

/// `s_validate_regex` for the symbol at `i`.
fn valid(symbols: &[Symbol], i: usize, symbol: &Symbol) -> bool {
    match symbol {
        Symbol::Star | Symbol::Plus => {
            let after_ok = symbols
                .get(i + 1)
                .is_none_or(|next| *next == Symbol::Char(b'-'));
            i > 0 && !matches!(symbols[i - 1], Symbol::Star | Symbol::Plus) && after_ok
        }
        Symbol::Alternation(variants) => {
            variants.len() <= 20
                && variants
                    .iter()
                    .all(|v| !v.is_empty() && v.iter().all(u8::is_ascii_alphanumeric))
                && variants.iter().enumerate().all(|(a, x)| {
                    variants[a + 1..]
                        .iter()
                        .all(|y| !x.starts_with(y) && !y.starts_with(x))
                })
        }
        _ => true,
    }
}
