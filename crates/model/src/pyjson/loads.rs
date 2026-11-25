//! `json.loads(text)`: CPython's scanner (`_json.c` `scan_once_unicode`,
//! `scanstring_unicode`, `_parse_object_unicode`, `_parse_array_unicode`,
//! `_match_number_unicode`) and `JSONDecoder.decode`. It takes `NaN`,
//! `Infinity` and `-Infinity`, reads an integer with `int()` and so its
//! 4300-digit limit, joins an escaped surrogate pair, and fails with
//! `JSONDecodeError`'s own text and position.
//!
//! A string with a lone surrogate (`"\ud800"`) is a [`Value::PyString`],
//! a key with one a [`crate::value::Key::Py`], as Python keeps it.
//!
//! The one thing a [`Value`] does not hold as Python does is a document
//! nested deeper than [`MAX_DEPTH`], a [`Limit`] of this reader rather than
//! a Python error. Past the limit the text is still scanned, without
//! building values, so text Python refuses fails with Python's error, and
//! only valid text nested that deep is the limit. Its callers decide what
//! the limit means by when it is met. Before anything is sent (a
//! paginator's starting token) the call is left to Python, which reads it.
//! After a request was sent (a response body) there is no going back to
//! Python, which would send it again, so the call fails, naming the limit.
//!
//! CPython's own scanner recurses on the C stack, and raises
//! `RecursionError` instead somewhere past 100,000 levels (1,000,000 do in
//! the CLI's CPython 3.14.8); this reader, which does not recurse past the
//! limit, reads any depth.
use crate::pyrepr::{float_from_text, int_from_text};
use crate::value::{Key, Map, Value};
use pytext::wtf8::{Text, Wtf8Buf};

/// How `json.loads` fails here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// A `ValueError`, with its text: `JSONDecodeError`'s, or `int()`'s
    /// digit limit.
    Invalid(String),
    /// JSON Python reads that this reader does not hold.
    Limit(Limit),
}

/// What Python reads and a [`Value`] does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// Arrays and objects nested deeper than [`MAX_DEPTH`].
    Depth,
}

impl Limit {
    /// The limit, named for a message.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Depth => "arrays and objects nested more than 900 deep",
        }
    }
}

impl std::fmt::Display for Limit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The deepest nesting of arrays and objects held, counting the outermost.
///
/// CPython's scanner reads a document nested far deeper (20,000 levels
/// decode in the CLI's CPython 3.14.8), but the CLI cannot print one past
/// 984: its JSON and text formatters recurse in Python, and a response
/// whose document member nests 984 deep under the top-level object fails
/// both with `RecursionError` (measured on the Python route: 984 levels
/// print, 985 fail). The limit stays 84 levels under that, so a document
/// held here is one Python prints whatever frames sit above its formatter,
/// and everything that walks a parsed [`Value`] (the readers, the
/// formatters, `Drop`) stays within a thread's default stack.
pub const MAX_DEPTH: usize = 900;

/// `json.loads(text)`.
///
/// # Errors
/// See [`LoadError`]. A document nested past [`MAX_DEPTH`] is the limit
/// only if Python reads it: invalid text fails with Python's error.
pub fn loads(text: &str) -> Result<Value, LoadError> {
    if text.starts_with('\u{feff}') {
        return Err(LoadError::Invalid(decode_error(
            "Unexpected UTF-8 BOM (decode using utf-8-sig)",
            text,
            0,
        )));
    }
    let mut reader = Reader {
        text,
        at: 0,
        depth: 0,
        deep: false,
    };
    reader.whitespace();
    let value = reader.value()?;
    reader.whitespace();
    if reader.at != text.len() {
        return Err(reader.error("Extra data", reader.at));
    }
    if reader.deep {
        return Err(LoadError::Limit(Limit::Depth));
    }
    Ok(value)
}

/// `JSONDecodeError(msg, doc, pos)`'s text, `at` being a byte offset: its
/// position, line and column count characters, as Python's `str` does.
fn decode_error(message: &str, text: &str, at: usize) -> String {
    let before = text.get(..at).unwrap_or(text);
    let position = before.chars().count();
    let line = before.matches('\n').count() + 1;
    let column = before.rfind('\n').map_or(position + 1, |newline| {
        position - before[..newline].chars().count()
    });
    format!("{message}: line {line} column {column} (char {position})")
}

struct Reader<'t> {
    text: &'t str,
    at: usize,
    depth: usize,
    /// Nesting past [`MAX_DEPTH`] was scanned, which only matters once the
    /// whole document loads: Python would fail on what follows first.
    deep: bool,
}

impl Reader<'_> {
    fn error(&self, message: &str, at: usize) -> LoadError {
        LoadError::Invalid(decode_error(message, self.text, at))
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn rest(&self) -> &[u8] {
        self.text.as_bytes().get(self.at..).unwrap_or_default()
    }

    /// Takes `byte` if it is next.
    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.at += 1;
        }
        found
    }

    /// Takes `word` if it is next.
    fn word(&mut self, word: &str) -> bool {
        let found = self.rest().starts_with(word.as_bytes());
        if found {
            self.at += word.len();
        }
        found
    }

    /// `IS_WHITESPACE`: `[ \t\n\r]*`.
    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// `scan_once_unicode`; where no value starts, the `StopIteration` that
    /// `raw_decode` and the containers raise as `Expecting value`.
    fn value(&mut self) -> Result<Value, LoadError> {
        match self.peek() {
            Some(b'"') => self.string().map(Value::from_text),
            Some(b'{') => self.nested(Self::object),
            Some(b'[') => self.nested(Self::array),
            _ => match self.constant() {
                Some(value) => Ok(value),
                None => self.number(),
            },
        }
    }

    /// `null`, `true`, `false`, and `parse_constant`'s three.
    fn constant(&mut self) -> Option<Value> {
        let constants = [
            ("null", Value::Null),
            ("true", Value::Bool(true)),
            ("false", Value::Bool(false)),
            ("NaN", Value::Float(f64::NAN)),
            ("Infinity", Value::Float(f64::INFINITY)),
            ("-Infinity", Value::Float(f64::NEG_INFINITY)),
        ];
        constants
            .into_iter()
            .find(|(word, _)| self.word(word))
            .map(|(_, value)| value)
    }

    /// An object or array, within `_Py_EnterRecursiveCall`. One past
    /// [`MAX_DEPTH`] is [`Self::skip`]ped, so the reader never recurses
    /// deeper than that.
    fn nested(
        &mut self,
        read: fn(&mut Self) -> Result<Value, LoadError>,
    ) -> Result<Value, LoadError> {
        if self.depth == MAX_DEPTH {
            self.skip()?;
            self.deep = true;
            return Ok(Value::Null);
        }
        self.depth += 1;
        let value = read(self);
        self.depth -= 1;
        value
    }

    /// `_parse_object_unicode`: a key seen again keeps its place and takes
    /// the later value, as a dict does.
    fn object(&mut self) -> Result<Value, LoadError> {
        self.at += 1;
        self.whitespace();
        let mut map = Map::new();
        if self.eat(b'}') {
            return Ok(Value::Map(map));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(
                    self.error("Expecting property name enclosed in double quotes", self.at)
                );
            }
            let key = self.key()?;
            let value = self.value()?;
            map.insert_key(key, value);
            self.whitespace();
            if self.eat(b'}') {
                return Ok(Value::Map(map));
            }
            let comma = self.at;
            if !self.eat(b',') {
                return Err(self.error("Expecting ',' delimiter", self.at));
            }
            self.whitespace();
            if self.peek() == Some(b'}') {
                return Err(self.error("Illegal trailing comma before end of object", comma));
            }
        }
    }

    /// An object's key, its `:` and the whitespace around it, at the
    /// key's opening quote.
    fn key(&mut self) -> Result<Key, LoadError> {
        let key = Key::from(self.string()?);
        self.whitespace();
        if !self.eat(b':') {
            return Err(self.error("Expecting ':' delimiter", self.at));
        }
        self.whitespace();
        Ok(key)
    }

    /// The object or array at `self.at` scanned as [`Self::object`] and
    /// [`Self::array`] read it, with their errors at their positions, but
    /// without recursing or keeping what it reads: a stack of the open
    /// brackets stands for the recursion.
    fn skip(&mut self) -> Result<(), LoadError> {
        let mut open: Vec<u8> = Vec::new();
        loop {
            // A value starts here.
            let opened = match self.peek() {
                Some(bracket @ (b'{' | b'[')) => {
                    self.at += 1;
                    self.whitespace();
                    let close = if bracket == b'{' { b'}' } else { b']' };
                    if self.eat(close) {
                        false
                    } else {
                        open.push(bracket);
                        if bracket == b'{' {
                            self.skip_key()?;
                        }
                        true
                    }
                }
                Some(b'"') => {
                    let _skipped = self.string()?;
                    false
                }
                _ => {
                    if self.constant().is_none() {
                        self.number()?;
                    }
                    false
                }
            };
            if opened {
                continue;
            }
            // A value ended: close what it ends, up to the next item.
            loop {
                let Some(&bracket) = open.last() else {
                    return Ok(());
                };
                self.whitespace();
                let close = if bracket == b'{' { b'}' } else { b']' };
                if self.eat(close) {
                    open.pop();
                    continue;
                }
                let comma = self.at;
                if !self.eat(b',') {
                    return Err(self.error("Expecting ',' delimiter", self.at));
                }
                self.whitespace();
                if self.peek() == Some(close) {
                    let kind = if bracket == b'{' { "object" } else { "array" };
                    return Err(self.error(
                        &format!("Illegal trailing comma before end of {kind}"),
                        comma,
                    ));
                }
                if bracket == b'{' {
                    self.skip_key()?;
                }
                break;
            }
        }
    }

    /// [`Self::key`] of a skipped object, where a key must start.
    fn skip_key(&mut self) -> Result<(), LoadError> {
        if self.peek() != Some(b'"') {
            return Err(self.error("Expecting property name enclosed in double quotes", self.at));
        }
        self.key().map(drop)
    }

    /// `_parse_array_unicode`.
    fn array(&mut self) -> Result<Value, LoadError> {
        self.at += 1;
        self.whitespace();
        let mut items = Vec::new();
        if self.eat(b']') {
            return Ok(Value::List(items));
        }
        loop {
            items.push(self.value()?);
            self.whitespace();
            if self.eat(b']') {
                return Ok(Value::List(items));
            }
            let comma = self.at;
            if !self.eat(b',') {
                return Err(self.error("Expecting ',' delimiter", self.at));
            }
            self.whitespace();
            if self.peek() == Some(b']') {
                return Err(self.error("Illegal trailing comma before end of array", comma));
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - start
    }

    /// `_match_number_unicode`: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`,
    /// a `float()` with a fraction or exponent, else an `int()`. Where no
    /// number starts, `Expecting value`.
    fn number(&mut self) -> Result<Value, LoadError> {
        let start = self.at;
        self.eat(b'-');
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => {
                self.at = start;
                return Err(self.error("Expecting value", start));
            }
        }
        let mut float = false;
        if self.rest().first() == Some(&b'.') && self.rest().get(1).is_some_and(u8::is_ascii_digit)
        {
            self.at += 1;
            self.digits();
            float = true;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let exponent = self.at;
            self.at += 1;
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if self.digits() == 0 {
                self.at = exponent;
            } else {
                float = true;
            }
        }
        let number = self.text.get(start..self.at).unwrap_or_default();
        if float {
            return Ok(Value::Float(float_from_text(number).unwrap_or(f64::NAN)));
        }
        int_from_text(number).map_err(LoadError::Invalid)
    }

    /// `scanstring_unicode`, strict: no control character unescaped. The
    /// text is a `String` unless it holds a lone surrogate.
    fn string(&mut self) -> Result<Text, LoadError> {
        let begin = self.at;
        self.at += 1;
        let mut out = Out::default();
        loop {
            let start = self.at;
            while self
                .peek()
                .is_some_and(|byte| byte != b'"' && byte != b'\\' && byte > 0x1F)
            {
                self.at += 1;
            }
            // Each stop is at an ASCII byte, so a character boundary.
            out.push_str(self.text.get(start..self.at).unwrap_or_default());
            match self.peek() {
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out.finish());
                }
                Some(b'\\') => {
                    self.at += 1;
                    if self.peek().is_none() {
                        return Err(self.error("Unterminated string starting at", begin));
                    }
                    if self.eat(b'u') {
                        self.unicode(&mut out)?;
                    } else {
                        out.push(self.escape()?);
                    }
                }
                Some(_) => return Err(self.error("Invalid control character at", self.at)),
                None => return Err(self.error("Unterminated string starting at", begin)),
            }
        }
    }

    /// Four hex digits, either case; `u` is the escape's `u`, where CPython
    /// reports a bad one.
    fn hex4(&mut self, u: usize) -> Result<u32, LoadError> {
        let digits = self
            .rest()
            .get(..4)
            .ok_or_else(|| self.error("Invalid \\uXXXX escape", u))?;
        let mut unit = 0;
        for digit in digits {
            let value = char::from(*digit)
                .to_digit(16)
                .ok_or_else(|| self.error("Invalid \\uXXXX escape", u))?;
            unit = unit << 4 | value;
        }
        self.at += 4;
        Ok(unit)
    }

    /// `\uXXXX`, a high surrogate joined with an escaped low one after it
    /// when one follows with a character after that; any other surrogate
    /// is kept alone, as Python's `str` keeps it.
    fn unicode(&mut self, out: &mut Out) -> Result<(), LoadError> {
        let unit = self.hex4(self.at - 1)?;
        let mut code = unit;
        if (0xD800..0xDC00).contains(&unit)
            && self.at + 6 < self.text.len()
            && self.rest().starts_with(b"\\u")
        {
            let pair = self.at;
            self.at += 2;
            let low = self.hex4(pair + 1)?;
            if (0xDC00..0xE000).contains(&low) {
                code = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
            } else {
                self.at = pair;
            }
        }
        match char::from_u32(code) {
            Some(character) => out.push(character),
            None => out.push_surrogate(code),
        }
        Ok(())
    }

    /// The character a backslash escape other than `\u` stands for.
    fn escape(&mut self) -> Result<char, LoadError> {
        let escaped = match self.peek() {
            Some(b'"') => '"',
            Some(b'\\') => '\\',
            Some(b'/') => '/',
            Some(b'b') => '\u{8}',
            Some(b'f') => '\u{c}',
            Some(b'n') => '\n',
            Some(b'r') => '\r',
            Some(b't') => '\t',
            _ => return Err(self.error("Invalid \\escape", self.at - 1)),
        };
        self.at += 1;
        Ok(escaped)
    }
}

/// A string being read: a `String` until a lone surrogate comes, then
/// text in `pytext::wtf8`'s encoding.
#[derive(Default)]
struct Out {
    text: String,
    wide: Option<Vec<u8>>,
}

impl Out {
    fn push_str(&mut self, text: &str) {
        match &mut self.wide {
            None => self.text.push_str(text),
            Some(bytes) => bytes.extend_from_slice(text.as_bytes()),
        }
    }

    fn push(&mut self, character: char) {
        match &mut self.wide {
            None => self.text.push(character),
            Some(bytes) => pytext::wtf8::push_code_point(bytes, u32::from(character)),
        }
    }

    fn push_surrogate(&mut self, point: u32) {
        let bytes = self
            .wide
            .get_or_insert_with(|| std::mem::take(&mut self.text).into_bytes());
        pytext::wtf8::push_code_point(bytes, point);
    }

    fn finish(self) -> Text {
        match self.wide {
            None => Ok(self.text),
            Some(bytes) => Wtf8Buf::from_bytes(bytes),
        }
    }
}

#[cfg(test)]
mod tests;
