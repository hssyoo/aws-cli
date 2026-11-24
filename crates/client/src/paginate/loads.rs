//! `json.loads` as `TokenDecoder.decode` reads a starting token: CPython's
//! scanner (`_json.c` `scan_once_unicode`, `scanstring_unicode`,
//! `_match_number_unicode`), which takes `NaN`, `Infinity` and `-Infinity`,
//! reads an integer with `int()` and so its 4300-digit limit, and joins an
//! escaped surrogate pair but keeps a lone surrogate.
use model::value::{Map, Value};

/// How the token's JSON fails to load.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum LoadError {
    /// A `ValueError`: `JSONDecodeError`, or `int()`'s digit limit.
    Invalid,
    /// JSON Python reads into what no [`Value`] holds, or nested deeper than
    /// this reader follows: Python's limit is its stack's, which it reports
    /// with the stack's size.
    Unsupported(&'static str),
}

/// The deepest nesting read. CPython 3.14 reads a token nested 1,000 deep
/// and fails on one 10,000 deep.
const MAX_DEPTH: usize = 200;

const LONE_SURROGATE: &str = "a lone surrogate";

/// `json.loads(text)`.
pub(super) fn loads(text: &str) -> Result<Value, LoadError> {
    let mut reader = Reader {
        text,
        at: 0,
        depth: 0,
        surrogate: false,
    };
    reader.whitespace();
    let value = reader.value()?;
    reader.whitespace();
    if reader.at != text.len() {
        return Err(LoadError::Invalid);
    }
    if reader.surrogate {
        return Err(LoadError::Unsupported(LONE_SURROGATE));
    }
    Ok(value)
}

struct Reader<'t> {
    text: &'t str,
    at: usize,
    depth: usize,
    /// A string held a lone surrogate, which only matters once the whole
    /// document loads.
    surrogate: bool,
}

impl Reader<'_> {
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

    /// `WHITESPACE`: `[ \t\n\r]*`.
    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// `scan_once_unicode`.
    fn value(&mut self) -> Result<Value, LoadError> {
        match self.peek() {
            Some(b'"') => self.string().map(Value::String),
            Some(b'{') => self.nested(Self::object),
            Some(b'[') => self.nested(Self::array),
            _ => self.constant().map_or_else(|| self.number(), Ok),
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

    /// An object or array, within `_Py_EnterRecursiveCall`.
    fn nested(
        &mut self,
        read: fn(&mut Self) -> Result<Value, LoadError>,
    ) -> Result<Value, LoadError> {
        if self.depth == MAX_DEPTH {
            return Err(LoadError::Unsupported("JSON nested too deep"));
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
        while self.peek() == Some(b'"') {
            let key = self.string()?;
            self.whitespace();
            if !self.eat(b':') {
                return Err(LoadError::Invalid);
            }
            self.whitespace();
            map.insert(key, self.value()?);
            self.whitespace();
            if self.eat(b'}') {
                return Ok(Value::Map(map));
            }
            if !self.eat(b',') {
                return Err(LoadError::Invalid);
            }
            self.whitespace();
        }
        Err(LoadError::Invalid)
    }

    /// `_parse_array_unicode`.
    fn array(&mut self) -> Result<Value, LoadError> {
        self.at += 1;
        self.whitespace();
        let mut items = Vec::new();
        if self.eat(b']') {
            return Ok(Value::List(items));
        }
        while self.peek().is_some() {
            items.push(self.value()?);
            self.whitespace();
            if self.eat(b']') {
                return Ok(Value::List(items));
            }
            if !self.eat(b',') {
                return Err(LoadError::Invalid);
            }
            self.whitespace();
        }
        Err(LoadError::Invalid)
    }

    fn digits(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        self.at - start
    }

    /// `_match_number_unicode`: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`,
    /// a `float()` with a fraction or exponent, else an `int()`.
    fn number(&mut self) -> Result<Value, LoadError> {
        let start = self.at;
        self.eat(b'-');
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(LoadError::Invalid),
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
            return Ok(Value::Float(
                model::pyrepr::float_from_text(number).unwrap_or(f64::NAN),
            ));
        }
        model::pyrepr::int_from_text(number).map_err(|_| LoadError::Invalid)
    }

    /// `scanstring_unicode`, strict: no control character unescaped.
    fn string(&mut self) -> Result<String, LoadError> {
        self.at += 1;
        let mut out = String::new();
        loop_until_quote(self, &mut out)?;
        Ok(out)
    }

    /// Four hex digits, either case.
    fn hex4(&mut self) -> Result<u32, LoadError> {
        let digits = self.rest().get(..4).ok_or(LoadError::Invalid)?;
        let mut unit = 0;
        for digit in digits {
            let value = char::from(*digit).to_digit(16).ok_or(LoadError::Invalid)?;
            unit = unit << 4 | value;
        }
        self.at += 4;
        Ok(unit)
    }

    /// `\uXXXX`, a high surrogate joined with an escaped low one after it
    /// when one follows with a character after that; any other surrogate
    /// is kept alone.
    fn unicode(&mut self, out: &mut String) -> Result<(), LoadError> {
        let unit = self.hex4()?;
        let mut code = unit;
        if (0xD800..0xDC00).contains(&unit)
            && self.at + 6 < self.text.len()
            && self.rest().starts_with(b"\\u")
        {
            let pair = self.at;
            self.at += 2;
            let low = self.hex4()?;
            if (0xDC00..0xE000).contains(&low) {
                code = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
            } else {
                self.at = pair;
            }
        }
        match char::from_u32(code) {
            Some(character) => out.push(character),
            None => {
                self.surrogate = true;
                out.push(char::REPLACEMENT_CHARACTER);
            }
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
            _ => return Err(LoadError::Invalid),
        };
        self.at += 1;
        Ok(escaped)
    }
}

/// The rest of a string after its opening quote, into `out`, and the
/// closing quote.
fn loop_until_quote(reader: &mut Reader<'_>, out: &mut String) -> Result<(), LoadError> {
    while reader.at < reader.text.len() {
        let start = reader.at;
        while reader
            .peek()
            .is_some_and(|byte| byte != b'"' && byte != b'\\' && byte > 0x1F)
        {
            reader.at += 1;
        }
        // Each stop is at an ASCII byte, so a character boundary.
        out.push_str(reader.text.get(start..reader.at).unwrap_or_default());
        match reader.peek() {
            Some(b'"') => {
                reader.at += 1;
                return Ok(());
            }
            Some(b'\\') => {
                reader.at += 1;
                if reader.eat(b'u') {
                    reader.unicode(out)?;
                } else {
                    out.push(reader.escape()?);
                }
            }
            _ => return Err(LoadError::Invalid),
        }
    }
    Err(LoadError::Invalid)
}

#[cfg(test)]
mod tests;
