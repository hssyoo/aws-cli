//! `json.dumps` byte for byte, with `sort_keys` off: botocore's JSON request
//! bodies and the CLI's JSON output are written with it. And `json.loads`
//! ([`loads`]), which reads every JSON response body and starting token.
use crate::pyrepr::float_repr;
use crate::value::Map;
use crate::value::Value;
use std::io::Write as _;

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

/// `json.dumps(value, indent=..., separators=..., ensure_ascii=..., default=...)`
/// as Rust text. With `ensure_ascii` (Python's default, and every request
/// body's and token's) the text is ASCII. Without it a lone surrogate is
/// written raw, which only [`dumps_wtf8`] holds; here it would be U+FFFD,
/// so the CLI's own output, which writes them, goes through that.
pub fn dumps(
    value: &Value,
    options: &Options,
    fallback: Option<Fallback<'_>>,
) -> Result<String, DumpError> {
    let bytes = dumps_wtf8(value, options, fallback)?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => pytext::wtf8::Wtf8Buf::from_bytes(error.into_bytes())
            .unwrap_or_else(|text| text.to_string_lossy()),
    })
}

/// `json.dumps(...)` as text in `pytext::wtf8`'s encoding: a lone surrogate
/// in a string or key, written raw without `ensure_ascii`, kept.
pub fn dumps_wtf8(
    value: &Value,
    options: &Options,
    fallback: Option<Fallback<'_>>,
) -> Result<Vec<u8>, DumpError> {
    let mut out = Vec::new();
    let mut sink = |chunk: &[u8]| -> Result<(), std::convert::Infallible> {
        out.extend_from_slice(chunk);
        Ok(())
    };
    match dump_chunks(value, options, fallback, &mut sink) {
        Ok(()) => Ok(out),
        Err(ChunkError::Dump(error)) => Err(error),
        Err(ChunkError::Write(never)) => match never {},
    }
}

/// Why [`dump_chunks`] stopped: the dump's own error, or the sink's.
#[derive(Debug, PartialEq, Eq)]
pub enum ChunkError<E> {
    Dump(DumpError),
    Write(E),
}

/// `json.dump(value, fp, ...)`: each chunk `JSONEncoder.iterencode` yields
/// (`_make_iterencode`, as `json.dump` never takes the one-shot C encoder)
/// handed to `sink` as `fp.write` takes it, in `pytext::wtf8`'s encoding.
/// So an error in a later chunk, the sink's or the dump's, finds the
/// earlier ones written, and a position the sink reports is within its
/// chunk, as Python's are.
///
/// The chunks: a scalar a list holds joined to the text before it (`[`
/// and the indentation, or the separator); a container or a value
/// `default` converts after that text, alone; a dict's key, its separator
/// and a scalar value each alone; the closing indentation and bracket
/// apart.
///
/// # Errors
/// The dump's `TypeError`, or the first error `sink` returns.
pub fn dump_chunks<E>(
    value: &Value,
    options: &Options,
    fallback: Option<Fallback<'_>>,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), ChunkError<E>> {
    let (item, key) = options.separators();
    let mut writer = Writer {
        chunk: Vec::new(),
        sink,
        indent: options.indent.as_deref(),
        item,
        key,
        ensure_ascii: options.ensure_ascii,
        fallback,
    };
    writer.value(value, 0)
}

struct Writer<'a, 's, E> {
    /// The chunk being built, in `pytext::wtf8`'s encoding.
    chunk: Vec<u8>,
    sink: &'s mut dyn FnMut(&[u8]) -> Result<(), E>,
    indent: Option<&'a str>,
    item: &'a str,
    key: &'a str,
    ensure_ascii: bool,
    fallback: Option<Fallback<'a>>,
}

impl<E> Writer<'_, '_, E> {
    fn put(&mut self, text: &str) {
        self.chunk.extend_from_slice(text.as_bytes());
    }

    /// `yield`: the chunk built so far, to the sink.
    fn emit(&mut self) -> Result<(), ChunkError<E>> {
        let result = (self.sink)(&self.chunk);
        self.chunk.clear();
        result.map_err(ChunkError::Write)
    }

    /// Adds `value` to the chunk if it is a scalar, which
    /// `_iterencode_list` joins to the text before it; `false` for a
    /// container or a value `default` converts.
    fn scalar(&mut self, value: &Value) -> bool {
        match value {
            Value::Null => self.put("null"),
            Value::Bool(true) => self.put("true"),
            Value::Bool(false) => self.put("false"),
            Value::Integer(number) => self.put(&number.to_string()),
            Value::BigInteger(text) => self.put(text),
            Value::Float(number) => self.put(&json_float(*number)),
            Value::String(text) => self.string(text.as_bytes()),
            Value::PyString(text) => self.string(text.as_bytes()),
            Value::List(_) | Value::Map(_) | Value::Blob(_) | Value::Timestamp(_) => {
                return false;
            }
        }
        true
    }

    /// `_make_iterencode._iterencode`, with nothing in the chunk.
    fn value(&mut self, value: &Value, level: usize) -> Result<(), ChunkError<E>> {
        if self.scalar(value) {
            return self.emit();
        }
        match value {
            Value::List(items) => self.list(items, level),
            Value::Map(map) => self.map(map, level),
            other => {
                let converted = match self.fallback {
                    Some(fallback) => fallback(other).map_err(ChunkError::Dump)?,
                    None => return Err(ChunkError::Dump(DumpError::unserializable(other))),
                };
                self.value(&converted, level)
            }
        }
    }

    /// `_iterencode_list`.
    fn list(&mut self, items: &[Value], level: usize) -> Result<(), ChunkError<E>> {
        if items.is_empty() {
            self.put("[]");
            return self.emit();
        }
        self.put("[");
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                let separator = self.item;
                self.put(separator);
            }
            self.newline(level + 1);
            if !self.scalar(item) {
                self.emit()?;
                self.value(item, level + 1)?;
                continue;
            }
            self.emit()?;
        }
        if self.indent.is_some() {
            self.newline(level);
            self.emit()?;
        }
        self.put("]");
        self.emit()
    }

    /// `_iterencode_dict`, with `sort_keys` off.
    fn map(&mut self, map: &Map, level: usize) -> Result<(), ChunkError<E>> {
        if map.is_empty() {
            self.put("{}");
            return self.emit();
        }
        self.put("{");
        self.emit()?;
        for (index, (key, value)) in map.iter().enumerate() {
            if index > 0 {
                let separator = self.item;
                self.put(separator);
            }
            self.newline(level + 1);
            // `item_separator`, or the first item's `newline_indent`.
            if index > 0 || self.indent.is_some() {
                self.emit()?;
            }
            self.string(key.as_bytes());
            self.emit()?;
            let separator = self.key;
            self.put(separator);
            self.emit()?;
            self.value(value, level + 1)?;
        }
        if self.indent.is_some() {
            self.newline(level);
            self.emit()?;
        }
        self.put("}");
        self.emit()
    }

    /// The newline and indentation of `level`, when indenting.
    fn newline(&mut self, level: usize) {
        if let Some(indent) = self.indent {
            self.put("\n");
            for _ in 0..level {
                self.put(indent);
            }
        }
    }

    /// `py_encode_basestring_ascii` or `py_encode_basestring` of `text`, in
    /// `pytext::wtf8`'s encoding: a lone surrogate is `\udXXX` with
    /// `ensure_ascii`, and raw without.
    fn string(&mut self, text: &[u8]) {
        self.put("\"");
        for point in pytext::wtf8::code_points(text) {
            match point {
                0x22 => self.put("\\\""),
                0x5C => self.put("\\\\"),
                0x0A => self.put("\\n"),
                0x0D => self.put("\\r"),
                0x09 => self.put("\\t"),
                0x08 => self.put("\\b"),
                0x0C => self.put("\\f"),
                0x20..=0x7E => pytext::wtf8::push_code_point(&mut self.chunk, point),
                _ if point < 0x20 || self.ensure_ascii => escape_utf16(&mut self.chunk, point),
                _ => pytext::wtf8::push_code_point(&mut self.chunk, point),
            }
        }
        self.put("\"");
    }
}

/// `\uXXXX` of each UTF-16 unit of `point`: a pair for one past U+FFFF, and
/// a lone surrogate as itself.
fn escape_utf16(out: &mut Vec<u8>, point: u32) {
    let units = match point {
        0x10000.. => {
            let offset = point - 0x10000;
            vec![0xD800 + (offset >> 10), 0xDC00 + (offset & 0x3FF)]
        }
        _ => vec![point],
    };
    for unit in units {
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
