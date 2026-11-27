//! Template strings, split at decode time exactly as the CRT engine's
//! resolver walks them (`aws_byte_buf_init_from_resolved_templated_string`):
//! `{name}` and `{name#path}` are references, `{{` is `{` and, before a
//! reference, `}}` is `}` and a lone `}` an error. Text after the last
//! `{` is kept as written, `}}` included, as the CRT engine keeps it.
//!
//! In JSON mode (an endpoint's `properties` text) only text inside JSON
//! strings is a template; outside them braces are the document's own.
//! Like the CRT engine, the mode counts quotes not preceded by `\` in each
//! piece it appends.
use crate::error::DecodeError;
use crate::ir::TemplatePart;

struct Out {
    parts: Vec<TemplatePart>,
    text: String,
    quotes: usize,
    json: bool,
}

impl Out {
    fn push(&mut self, piece: &str) {
        if self.json {
            let bytes = piece.as_bytes();
            self.quotes += (0..bytes.len())
                .filter(|&i| bytes[i] == b'"' && !(i > 0 && bytes[i - 1] == b'\\'))
                .count();
        }
        self.text.push_str(piece);
    }
    fn in_string(&self) -> bool {
        self.quotes % 2 == 1
    }
    fn reference(&mut self, part: TemplatePart) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.parts.push(TemplatePart::Lit(text.into()));
        }
        self.parts.push(part);
    }
}

/// Splits `text` into literal pieces and the references `resolve` makes of
/// each `{…}`.
pub(crate) fn split(
    text: &str,
    json: bool,
    mut resolve: impl FnMut(&str) -> Result<TemplatePart, DecodeError>,
) -> Result<Box<[TemplatePart]>, DecodeError> {
    let mut out = Out {
        parts: Vec::new(),
        text: String::new(),
        quotes: usize::from(!json),
        json,
    };
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        prefix(&mut out, &rest[..open])?;
        let tail = &rest[open..];
        if !out.in_string() || tail.starts_with("{{") {
            out.text.push('{');
            rest = &tail[if out.in_string() { 2 } else { 1 }..];
            continue;
        }
        let inner = &tail[1..];
        let close = inner
            .find('}')
            .ok_or_else(|| DecodeError::rule_set(format!("unclosed template in {text:?}")))?;
        out.reference(resolve(&inner[..close])?);
        rest = &inner[close + 1..];
    }
    out.push(rest);
    if !out.text.is_empty() {
        out.parts.push(TemplatePart::Lit(out.text.into()));
    }
    Ok(out.parts.into())
}

/// The text before a `{`: inside a string `}}` is `}` and a lone `}` an
/// error.
fn prefix(out: &mut Out, mut text: &str) -> Result<(), DecodeError> {
    while let Some(close) = text.find('}') {
        out.push(&text[..close]);
        let tail = &text[close + 1..];
        out.text.push('}');
        text = match tail.strip_prefix('}') {
            _ if !out.in_string() => tail,
            Some(after) => after,
            None => return Err(DecodeError::rule_set(format!("unmatched `}}` in {text:?}"))),
        };
    }
    out.push(text);
    Ok(())
}
