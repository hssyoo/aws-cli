//! Python's numbers as the functions use them: `int`, unbounded, and
//! `float`, with Python's exact comparisons between the two, its `+`, and
//! CPython 3.14's `sum()`.
use crate::error::Error;
use std::cmp::Ordering;

/// A Python `int` or `float`. An `int` within `i64` is always `Int`.
#[derive(Clone, Debug, PartialEq)]
pub enum Number {
    Int(i64),
    /// An `int` outside `i64`.
    Big(BigInt),
    Float(f64),
}

/// An integer outside `i64`: a sign and base-10⁹ limbs, least significant
/// first, with no zero limb last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BigInt {
    negative: bool,
    limbs: Vec<u32>,
}

const BASE: u64 = 1_000_000_000;

impl BigInt {
    /// The integer of decimal `text`, an optional `-` and digits.
    pub fn parse(text: &str) -> Option<Self> {
        let (negative, digits) = text
            .strip_prefix('-')
            .map_or((false, text), |rest| (true, rest));
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let mut limbs = Vec::with_capacity(digits.len() / 9 + 1);
        let mut end = digits.len();
        while end > 0 {
            let start = end.saturating_sub(9);
            limbs.push(digits[start..end].parse().ok()?);
            end = start;
        }
        Some(Self { negative, limbs }.normalized())
    }

    fn from_i128(value: i128) -> Self {
        let mut magnitude = value.unsigned_abs();
        let mut limbs = Vec::new();
        while magnitude > 0 {
            limbs.push((magnitude % u128::from(BASE)) as u32);
            magnitude /= u128::from(BASE);
        }
        Self {
            negative: value < 0,
            limbs,
        }
    }

    fn normalized(mut self) -> Self {
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
        if self.limbs.is_empty() {
            self.negative = false;
        }
        self
    }

    /// The integer as a [`Number`]: `Int` when it fits.
    fn into_number(self) -> Number {
        match i64::try_from(self.to_i128()) {
            Ok(value) if self.limbs.len() <= 3 => Number::Int(value),
            _ => Number::Big(self),
        }
    }

    /// The value when at most three limbs, else a meaningless saturation.
    fn to_i128(&self) -> i128 {
        if self.limbs.len() > 3 {
            return if self.negative { i128::MIN } else { i128::MAX };
        }
        let magnitude = self.limbs.iter().rev().fold(0i128, |acc, &limb| {
            acc * i128::from(BASE as u32) + i128::from(limb)
        });
        if self.negative { -magnitude } else { magnitude }
    }

    fn cmp_magnitude(&self, other: &Self) -> Ordering {
        self.limbs
            .len()
            .cmp(&other.limbs.len())
            .then_with(|| self.limbs.iter().rev().cmp(other.limbs.iter().rev()))
    }

    fn compare(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.cmp_magnitude(other),
            (true, true) => other.cmp_magnitude(self),
        }
    }

    fn add(&self, other: &Self) -> Self {
        if self.negative == other.negative {
            let mut limbs = Vec::with_capacity(self.limbs.len().max(other.limbs.len()) + 1);
            let mut carry = 0u64;
            for i in 0..self.limbs.len().max(other.limbs.len()) {
                let sum = u64::from(*self.limbs.get(i).unwrap_or(&0))
                    + u64::from(*other.limbs.get(i).unwrap_or(&0))
                    + carry;
                limbs.push((sum % BASE) as u32);
                carry = sum / BASE;
            }
            limbs.push(carry as u32);
            return Self {
                negative: self.negative,
                limbs,
            }
            .normalized();
        }
        let (large, small) = match self.cmp_magnitude(other) {
            Ordering::Less => (other, self),
            _ => (self, other),
        };
        let mut limbs = Vec::with_capacity(large.limbs.len());
        let mut borrow = 0i64;
        for (i, &limb) in large.limbs.iter().enumerate() {
            let mut difference =
                i64::from(limb) - i64::from(*small.limbs.get(i).unwrap_or(&0)) - borrow;
            borrow = i64::from(difference < 0);
            if difference < 0 {
                difference += BASE as i64;
            }
            limbs.push(difference as u32);
        }
        Self {
            negative: large.negative,
            limbs,
        }
        .normalized()
    }

    fn negated(&self) -> Self {
        Self {
            negative: !self.negative,
            limbs: self.limbs.clone(),
        }
        .normalized()
    }

    /// `float(self)`, correctly rounded; `None` past `float`'s range.
    fn to_f64(&self) -> Option<f64> {
        self.to_string()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
    }

    /// The integer `value`, finite and integral, exactly.
    fn from_integral(value: f64) -> Self {
        if value.abs() < 1e38 {
            return Self::from_i128(value as i128);
        }
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32 - 1075;
        let mut result = Self::from_i128(i128::from(bits & ((1 << 52) - 1) | (1 << 52)));
        for _ in 0..exponent {
            let mut carry = 0u64;
            for limb in &mut result.limbs {
                let doubled = u64::from(*limb) * 2 + carry;
                *limb = (doubled % BASE) as u32;
                carry = doubled / BASE;
            }
            if carry > 0 {
                result.limbs.push(carry as u32);
            }
        }
        result.negative = value < 0.0;
        result
    }
}

impl std::fmt::Display for BigInt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        let mut limbs = self.limbs.iter().rev();
        write!(f, "{}", limbs.next().unwrap_or(&0))?;
        limbs.try_for_each(|limb| write!(f, "{limb:09}"))
    }
}

impl Number {
    /// The `int` of canonical decimal `text`.
    pub fn from_int_text(text: &str) -> Option<Self> {
        BigInt::parse(text).map(BigInt::into_number)
    }

    fn big(&self) -> Option<BigInt> {
        match self {
            Self::Int(value) => Some(BigInt::from_i128(i128::from(*value))),
            Self::Big(value) => Some(value.clone()),
            Self::Float(_) => None,
        }
    }

    /// `float(self)`: Python's conversion of an `int`, which fails past
    /// `float`'s range.
    pub(crate) fn to_f64(&self) -> Result<f64, Error> {
        match self {
            Self::Int(value) => Ok(*value as f64),
            Self::Big(value) => value
                .to_f64()
                .ok_or_else(|| Error::new("OverflowError", "int too large to convert to float")),
            Self::Float(value) => Ok(*value),
        }
    }

    /// `self == 0 or self == 1`, the values `True` and `False` equal.
    pub(crate) fn is_zero_or_one(&self) -> bool {
        match self {
            Self::Int(value) => matches!(value, 0 | 1),
            Self::Big(_) => false,
            Self::Float(value) => *value == 0.0 || *value == 1.0,
        }
    }

    /// Python's comparison: exact between `int` and `float`, `None` when
    /// a `NaN` is involved.
    pub(crate) fn compare(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Int(a), Self::Int(b)) => Some(a.cmp(b)),
            (Self::Float(a), Self::Float(b)) => a.partial_cmp(b),
            (Self::Float(_), _) => other.compare(self).map(Ordering::reverse),
            (int, Self::Float(float)) => compare_int_float(int, *float),
            (a, b) => Some(a.big()?.compare(&b.big()?)),
        }
    }

    /// Python's `self + other`.
    pub(crate) fn add(&self, other: &Self) -> Result<Self, Error> {
        if let (Self::Int(a), Self::Int(b)) = (self, other)
            && let Some(sum) = a.checked_add(*b)
        {
            return Ok(Self::Int(sum));
        }
        match (self.big(), other.big()) {
            (Some(a), Some(b)) => Ok(a.add(&b).into_number()),
            _ => Ok(Self::Float(self.to_f64()? + other.to_f64()?)),
        }
    }

    /// `abs(self)`.
    pub(crate) fn abs(&self) -> Self {
        match self {
            Self::Int(value) => value.checked_abs().map_or_else(
                || {
                    BigInt::from_i128(i128::from(*value))
                        .negated()
                        .into_number()
                },
                Self::Int,
            ),
            Self::Big(value) if value.negative => Self::Big(value.negated()),
            Self::Big(_) => self.clone(),
            Self::Float(value) => Self::Float(value.abs()),
        }
    }

    /// `math.ceil(self)` (`up`) or `math.floor(self)`: an `int`.
    pub(crate) fn round(&self, up: bool) -> Result<Self, Error> {
        let Self::Float(value) = self else {
            return Ok(self.clone());
        };
        if value.is_nan() {
            return Err(Error::new(
                "ValueError",
                "cannot convert float NaN to integer",
            ));
        }
        if value.is_infinite() {
            return Err(Error::new(
                "OverflowError",
                "cannot convert float infinity to integer",
            ));
        }
        let rounded = if up { value.ceil() } else { value.floor() };
        Ok(BigInt::from_integral(rounded).into_number())
    }
}

fn compare_int_float(int: &Number, float: f64) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    if float.is_infinite() {
        return Some(if float > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let whole = float.trunc();
    let integral = BigInt::from_integral(whole);
    match int.big()?.compare(&integral) {
        Ordering::Equal => 0.0f64.partial_cmp(&(float - whole)),
        order => Some(order),
    }
}

/// CPython 3.14's `sum(items)` of `int`s and `float`s: `int`s added in
/// `i64` until one overflows or a `float` comes, then `float`s with
/// Neumaier's compensation, and otherwise one `+` at a time.
pub(crate) fn sum(items: &[Number]) -> Result<Number, Error> {
    let mut items = items.iter();
    let mut total = 0i64;
    let mut result = loop {
        match items.next() {
            None => return Ok(Number::Int(total)),
            Some(Number::Int(item)) if total.checked_add(*item).is_some() => total += item,
            Some(item) => break Number::Int(total).add(item)?,
        }
    };
    if let Number::Float(start) = result {
        let (mut high, mut low) = (start, 0.0f64);
        for item in items {
            let x = item.to_f64()?;
            let t = high + x;
            low += if high.abs() >= x.abs() {
                (high - t) + x
            } else {
                (x - t) + high
            };
            high = t;
        }
        let total = if low != 0.0 && low.is_finite() {
            high + low
        } else {
            high
        };
        return Ok(Number::Float(total));
    }
    for item in items {
        result = result.add(item)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
