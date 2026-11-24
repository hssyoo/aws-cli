//! `json.dumps` byte for byte, with `sort_keys` off: botocore's JSON request
//! bodies and the CLI's JSON output are written with it. And `json.loads`
//! ([`loads`]), which reads every JSON response body and starting token.
use crate::pyrepr::float_repr;
use crate::value::Value;
use std::fmt::Write as _;

/// The `json.dumps` keywords this writer honours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// `indent`: the text one level adds; `None` writes everything on one line.
    pub indent: Option<String>,
    /// `separators=(item, key)`; `None` takes Python's defaults: `(', ', ': ')`,
    /// or `(',', ': ')` when `indent` is set.
    pub separators: Option<(String, String)>,
    /// `ensure_ascii`: escape every character outside printable ASCII.
    pub ensure_ascii: bool,
}

impl Default for Options {
    /// `json.dumps(value)`.
    fn default() -> Self {
        Self {
            indent: None,
            separators: None,
            ensure_ascii: true,
        }
    }
}

impl Options {
    /// `separators=(',', ':')`, which botocore's JSON serializer uses.
    pub fn compact() -> Self {
        Self {
            separators: Some((",".into(), ":".into())),
            ..Self::default()
        }
    }

    /// `indent=width`.
    pub fn indented(width: usize) -> Self {
        Self {
            indent: Some(" ".repeat(width)),
            ..Self::default()
        }
    }

    /// `JSONEncoder.__init__`'s choice of separators.
    fn separators(&self) -> (&str, &str) {
        match (&self.separators, &self.indent) {
            (Some((item, key)), _) => (item, key),
            (None, Some(_)) => (",", ": "),
            (None, None) => (", ", ": "),
        }
    }
}

/// `json.dumps`' `default`: converts a value JSON cannot hold, or fails.
pub type Fallback<'a> = &'a dyn Fn(&Value) -> Result<Value, DumpError>;

/// The `TypeError` `json.dumps` raises, with its message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DumpError(String);

impl DumpError {
    /// `JSONEncoder.default`'s error for a value it cannot encode.
    pub fn unserializable(value: &Value) -> Self {
        let name = match value {
            Value::Timestamp(_) => "datetime",
            _ => "bytes",
        };
        Self(format!("Object of type {name} is not JSON serializable"))
    }
}

impl std::fmt::Display for DumpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DumpError {}

/// `json.dumps(value, indent=..., separators=..., ensure_ascii=..., default=...)`.
pub fn dumps(
    value: &Value,
    options: &Options,
    fallback: Option<Fallback<'_>>,
) -> Result<String, DumpError> {
    let (item, key) = options.separators();
    let mut writer = Writer {
        out: String::new(),
        indent: options.indent.as_deref(),
        item,
        key,
        ensure_ascii: options.ensure_ascii,
        fallback,
    };
    writer.value(value, 0)?;
    Ok(writer.out)
}

struct Writer<'a> {
    out: String,
    indent: Option<&'a str>,
    item: &'a str,
    key: &'a str,
    ensure_ascii: bool,
    fallback: Option<Fallback<'a>>,
}

impl Writer<'_> {
    /// `_make_iterencode._iterencode`.
    fn value(&mut self, value: &Value, level: usize) -> Result<(), DumpError> {
        match value {
            Value::Null => self.out.push_str("null"),
            Value::Bool(true) => self.out.push_str("true"),
            Value::Bool(false) => self.out.push_str("false"),
            Value::Integer(number) => self.out.push_str(&number.to_string()),
            Value::BigInteger(text) => self.out.push_str(text),
            Value::Float(number) => self.out.push_str(&json_float(*number)),
            Value::String(text) => self.string(text),
            Value::List(items) => self.list(items, level)?,
            Value::Map(map) => self.map(map.iter(), map.is_empty(), level)?,
            Value::Blob(_) | Value::Timestamp(_) => {
                let converted = match self.fallback {
                    Some(fallback) => fallback(value)?,
                    None => return Err(DumpError::unserializable(value)),
                };
                self.value(&converted, level)?;
            }
        }
        Ok(())
    }

    fn list(&mut self, items: &[Value], level: usize) -> Result<(), DumpError> {
        if items.is_empty() {
            self.out.push_str("[]");
            return Ok(());
        }
        self.out.push('[');
        for (index, item) in items.iter().enumerate() {
            self.separator(index, level + 1);
            self.value(item, level + 1)?;
        }
        self.close(level);
        self.out.push(']');
        Ok(())
    }

    fn map<'v>(
        &mut self,
        entries: impl Iterator<Item = (&'v str, &'v Value)>,
        empty: bool,
        level: usize,
    ) -> Result<(), DumpError> {
        if empty {
            self.out.push_str("{}");
            return Ok(());
        }
        self.out.push('{');
        for (index, (key, value)) in entries.enumerate() {
            self.separator(index, level + 1);
            self.string(key);
            self.out.push_str(self.key);
            self.value(value, level + 1)?;
        }
        self.close(level);
        self.out.push('}');
        Ok(())
    }

    /// The item separator before every item but the first, then the newline
    /// and indentation of `level` when indenting.
    fn separator(&mut self, index: usize, level: usize) {
        if index > 0 {
            self.out.push_str(self.item);
        }
        self.newline(level);
    }

    fn close(&mut self, level: usize) {
        self.newline(level);
    }

    fn newline(&mut self, level: usize) {
        if let Some(indent) = self.indent {
            self.out.push('\n');
            for _ in 0..level {
                self.out.push_str(indent);
            }
        }
    }

    /// `py_encode_basestring_ascii` or `py_encode_basestring`.
    fn string(&mut self, text: &str) {
        self.out.push('"');
        for character in text.chars() {
            match character {
                '"' => self.out.push_str("\\\""),
                '\\' => self.out.push_str("\\\\"),
                '\n' => self.out.push_str("\\n"),
                '\r' => self.out.push_str("\\r"),
                '\t' => self.out.push_str("\\t"),
                '\u{8}' => self.out.push_str("\\b"),
                '\u{c}' => self.out.push_str("\\f"),
                ' '..='~' => self.out.push(character),
                c if c < ' ' || self.ensure_ascii => escape_utf16(&mut self.out, c),
                c => self.out.push(c),
            }
        }
        self.out.push('"');
    }
}

fn escape_utf16(out: &mut String, character: char) {
    let mut units = [0; 2];
    for unit in character.encode_utf16(&mut units) {
        let _ = write!(out, "\\u{unit:04x}");
    }
}

/// `float.__repr__`, with `JSONEncoder`'s `NaN`, `Infinity` and `-Infinity`.
fn json_float(number: f64) -> String {
    if number.is_nan() {
        "NaN".into()
    } else if number.is_infinite() {
        if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .into()
    } else {
        float_repr(number)
    }
}

mod loads;
pub use loads::{Limit, LoadError, MAX_DEPTH, loads};

#[cfg(test)]
mod tests;
