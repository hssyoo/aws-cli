//! An XML reader shaped like the `xml.etree.ElementTree` tree botocore
//! walks (`BaseXMLResponseParser._parse_xml_string_to_dom`): each element's
//! tag as ElementTree spells it (`{uri}local` in a namespace), attributes in
//! order, the text before its first child, and its children.
//!
//! It rejects what expat rejects in the documents services send, with
//! expat's message and position (`mismatched tag: line 1, column 5`), and
//! decodes UTF-8 whatever the declaration says, as botocore's parser is
//! told to. Two limits of its own: elements nest at most [`MAX_DEPTH`]
//! deep, and a DOCTYPE with an internal subset is refused.
use std::collections::HashMap;

/// The deepest element nesting read.
pub const MAX_DEPTH: usize = 512;
/// The message for a document nested deeper than [`MAX_DEPTH`].
pub const DEPTH_MESSAGE: &str = "element nesting too deep";
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// An element as ElementTree builds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Element {
    /// `{uri}local` for a name in a namespace, `local` otherwise.
    pub tag: String,
    /// Names spelled as `tag` is; `xmlns` declarations are not attributes.
    pub attributes: Vec<(String, String)>,
    /// Character data before the first child; `None` when there is none.
    pub text: Option<String>,
    pub children: Vec<Element>,
}

impl Element {
    /// The tag without its namespace (`BaseXMLResponseParser._node_tag`).
    pub fn local_name(&self) -> &str {
        strip_namespace(&self.tag)
    }
}

/// `re.sub('{.*}', '', name)`.
pub fn strip_namespace(name: &str) -> &str {
    match (name.find('{'), name.rfind('}')) {
        (Some(0), Some(end)) => &name[end + 1..],
        _ => name,
    }
}

/// expat's error, as `str(ParseError)` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XmlError {
    pub message: String,
}

impl std::fmt::Display for XmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for XmlError {}

/// What went wrong, before its position is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    NoElement,
    Unclosed,
    UnclosedCdata,
    Mismatched,
    Junk,
    InvalidToken,
    UndefinedEntity,
    BadCharacter,
    DuplicateAttribute,
    UnboundPrefix,
    MisplacedDeclaration,
    TooDeep,
}

impl Fault {
    const fn text(self) -> &'static str {
        match self {
            Self::NoElement => "no element found",
            Self::Unclosed => "unclosed token",
            Self::UnclosedCdata => "unclosed CDATA section",
            Self::Mismatched => "mismatched tag",
            Self::Junk => "junk after document element",
            Self::InvalidToken => "not well-formed (invalid token)",
            Self::UndefinedEntity => "undefined entity",
            Self::BadCharacter => "reference to invalid character number",
            Self::DuplicateAttribute => "duplicate attribute",
            Self::UnboundPrefix => "unbound prefix",
            Self::MisplacedDeclaration => "XML or text declaration not at start of entity",
            Self::TooDeep => DEPTH_MESSAGE,
        }
    }
}

/// An attribute as written: its name, its value, and where its name starts.
type RawAttribute = (String, String, usize);

/// A fault at a byte offset of the decoded text.
type Failure = (Fault, usize);
type Step<T> = Result<T, Failure>;

/// Parses a whole document.
///
/// # Errors
/// The document is not well-formed XML: expat's message.
pub fn parse(document: &[u8]) -> Result<Element, XmlError> {
    let (text, invalid) = match std::str::from_utf8(document) {
        Ok(text) => (text, None),
        Err(error) => {
            let end = error.valid_up_to();
            (
                std::str::from_utf8(&document[..end]).unwrap_or_default(),
                Some(end),
            )
        }
    };
    let mut reader = Reader {
        text,
        at: 0,
        invalid,
        scopes: Vec::new(),
    };
    reader.document().map_err(|(fault, at)| XmlError {
        message: describe(fault, text, at),
    })
}

/// `<fault>: line L, column C`, counting lines from 1 and characters from 0
/// as expat does.
fn describe(fault: Fault, text: &str, at: usize) -> String {
    if fault == Fault::TooDeep {
        return DEPTH_MESSAGE.into();
    }
    let before = &text[..at.min(text.len())];
    let line = 1 + before.matches('\n').count();
    let start = before.rfind('\n').map_or(0, |index| index + 1);
    let column = before[start..].chars().count();
    format!("{}: line {line}, column {column}", fault.text())
}

struct Reader<'a> {
    text: &'a str,
    at: usize,
    /// The offset of the first byte that is not UTF-8, where reading stops.
    invalid: Option<usize>,
    /// Namespace bindings per open element.
    scopes: Vec<HashMap<String, String>>,
}

impl Reader<'_> {
    fn rest(&self) -> &str {
        &self.text[self.at..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn eat(&mut self, prefix: &str) -> bool {
        let found = self.rest().starts_with(prefix);
        if found {
            self.at += prefix.len();
        }
        found
    }

    /// The failure for running out of text: an invalid byte, if that is
    /// what ended the decoded text, otherwise `fault`.
    fn end(&self, fault: Fault, at: usize) -> Failure {
        match self.invalid {
            Some(invalid) => (Fault::InvalidToken, invalid),
            None => (fault, at),
        }
    }

    fn skip_space(&mut self) {
        let space = self.rest().len() - self.rest().trim_start_matches(is_space).len();
        self.at += space;
    }

    /// The prolog, one element, then comments, processing instructions and
    /// space.
    fn document(&mut self) -> Step<Element> {
        self.eat("\u{feff}");
        if self.rest().starts_with("<?xml") && self.rest()[5..].starts_with(is_space) {
            self.instruction()?;
        }
        self.misc(false)?;
        if self.peek().is_none() {
            return Err(self.end(Fault::NoElement, self.at));
        }
        let root = self.element(0)?;
        self.misc(true)?;
        match self.peek() {
            None if self.invalid.is_none() => Ok(root),
            None => Err(self.end(Fault::Junk, self.at)),
            Some(_) => Err((Fault::Junk, self.at)),
        }
    }

    /// Space, comments, processing instructions and (before the root) a
    /// DOCTYPE; stops at anything else.
    fn misc(&mut self, after_root: bool) -> Step<()> {
        loop {
            self.skip_space();
            if self.rest().starts_with("<!--") {
                self.comment()?;
            } else if self.rest().starts_with("<?xml") && after_root {
                return Err((Fault::Junk, self.at));
            } else if self.rest().starts_with("<?xml") && self.rest()[5..].starts_with(is_space) {
                return Err((Fault::MisplacedDeclaration, self.at));
            } else if self.rest().starts_with("<?") {
                self.instruction()?;
            } else if !after_root && self.rest().starts_with("<!DOCTYPE") {
                self.doctype()?;
            } else if !after_root && self.peek().is_some_and(|c| c != '<') {
                let width = self.peek().map_or(1, char::len_utf8);
                return Err((Fault::InvalidToken, self.at + width));
            } else {
                return Ok(());
            }
        }
    }

    fn comment(&mut self) -> Step<()> {
        let start = self.at;
        self.at += 4;
        let Some(index) = self.rest().find("--") else {
            return Err(self.end(Fault::Unclosed, start));
        };
        self.check_chars(index)?;
        self.at += index + 2;
        if self.eat(">") {
            return Ok(());
        }
        if self.peek().is_none() {
            return Err(self.end(Fault::Unclosed, start));
        }
        Err((Fault::InvalidToken, self.at))
    }

    fn instruction(&mut self) -> Step<()> {
        let start = self.at;
        let Some(index) = self.rest().find("?>") else {
            return Err(self.end(Fault::Unclosed, start));
        };
        self.at += 2;
        self.check_chars(index - 2)?;
        self.at += index;
        Ok(())
    }

    /// `<!DOCTYPE ...>` without an internal subset.
    fn doctype(&mut self) -> Step<()> {
        let start = self.at;
        let Some(index) = self.rest().find('>') else {
            return Err(self.end(Fault::Unclosed, start));
        };
        if self.rest()[..index].contains('[') {
            return Err((Fault::InvalidToken, start));
        }
        self.at += index + 1;
        Ok(())
    }

    /// The next `count` bytes hold only XML characters.
    fn check_chars(&self, count: usize) -> Step<()> {
        let span = &self.rest()[..count.min(self.rest().len())];
        match span.char_indices().find(|(_, c)| !is_char(*c)) {
            Some((offset, _)) => Err((Fault::InvalidToken, self.at + offset)),
            None => Ok(()),
        }
    }

    /// An element starting at `<`, with everything inside it.
    fn element(&mut self, depth: usize) -> Step<Element> {
        if depth >= MAX_DEPTH {
            return Err((Fault::TooDeep, self.at));
        }
        let start = self.at;
        self.at += 1;
        let name = self.name(start)?;
        let (raw, empty) = self.attributes(start)?;
        let scope = self.bind(&raw);
        self.scopes.push(scope);
        let mut element = self.resolve(start, &name, raw)?;
        if !empty {
            self.content(&mut element, &name, depth)?;
        }
        self.scopes.pop();
        Ok(element)
    }

    /// A name at the cursor; `start` is where its token began.
    fn name(&mut self, start: usize) -> Step<String> {
        let Some(first) = self.peek() else {
            return Err(self.end(Fault::Unclosed, start));
        };
        if !is_name_start(first) {
            return Err((Fault::InvalidToken, self.at));
        }
        let length = self
            .rest()
            .find(|c: char| !is_name_char(c))
            .unwrap_or(self.rest().len());
        let name = self.rest()[..length].to_owned();
        self.at += length;
        Ok(name)
    }

    /// The attributes after a start tag's name, and whether the tag ends
    /// with `/>`.
    fn attributes(&mut self, start: usize) -> Step<(Vec<RawAttribute>, bool)> {
        let mut attributes: Vec<RawAttribute> = Vec::new();
        loop {
            let before = self.at;
            self.skip_space();
            if self.eat("/>") {
                return Ok((attributes, true));
            }
            if self.eat(">") {
                return Ok((attributes, false));
            }
            if self.peek().is_none() || self.rest() == "/" {
                return Err(self.end(Fault::Unclosed, start));
            }
            if self.at == before {
                return Err((Fault::InvalidToken, self.at));
            }
            let position = self.at;
            let name = self.name(start)?;
            if attributes.iter().any(|(other, _, _)| *other == name) {
                return Err((Fault::DuplicateAttribute, position));
            }
            let value = self.attribute_value(start)?;
            attributes.push((name, value, position));
        }
    }

    /// `= "value"` or `= 'value'`, references expanded and space normalized.
    fn attribute_value(&mut self, start: usize) -> Step<String> {
        self.skip_space();
        if !self.eat("=") {
            return self.missing(start);
        }
        self.skip_space();
        let Some(quote) = self.peek().filter(|c| matches!(c, '"' | '\'')) else {
            return self.missing(start);
        };
        self.at += 1;
        let mut value = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(self.end(Fault::Unclosed, start));
            };
            match c {
                _ if c == quote => {
                    self.at += 1;
                    return Ok(value);
                }
                '<' => return Err((Fault::InvalidToken, self.at)),
                '&' => value.push_str(&self.reference()?),
                '\r' => {
                    self.at += 1;
                    self.eat("\n");
                    value.push(' ');
                }
                '\t' | '\n' => {
                    self.at += 1;
                    value.push(' ');
                }
                c if !is_char(c) => return Err((Fault::InvalidToken, self.at)),
                c => {
                    self.at += c.len_utf8();
                    value.push(c);
                }
            }
        }
    }

    fn missing<T>(&self, start: usize) -> Step<T> {
        match self.peek() {
            None => Err(self.end(Fault::Unclosed, start)),
            Some(_) => Err((Fault::InvalidToken, self.at)),
        }
    }

    /// The namespace declarations among `attributes`.
    fn bind(&self, attributes: &[RawAttribute]) -> HashMap<String, String> {
        let mut scope = HashMap::new();
        for (name, value, _) in attributes {
            if name == "xmlns" {
                scope.insert(String::new(), value.clone());
            } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                scope.insert(prefix.to_owned(), value.clone());
            }
        }
        scope
    }

    /// The namespace `prefix` is bound to; `""` is the default namespace.
    fn lookup(&self, prefix: &str) -> Option<&str> {
        if prefix == "xml" {
            return Some(XML_NAMESPACE);
        }
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(prefix))
            .map(String::as_str)
    }

    /// The element with its names in ElementTree's spelling.
    fn resolve(&self, start: usize, name: &str, raw: Vec<RawAttribute>) -> Step<Element> {
        let tag = self
            .expand(name, true)
            .ok_or((Fault::UnboundPrefix, start))?;
        let mut attributes = Vec::with_capacity(raw.len());
        for (attribute, value, _) in raw {
            if attribute == "xmlns" || attribute.starts_with("xmlns:") {
                continue;
            }
            let name = self
                .expand(&attribute, false)
                .ok_or((Fault::UnboundPrefix, start))?;
            attributes.push((name, value));
        }
        Ok(Element {
            tag,
            attributes,
            ..Element::default()
        })
    }

    /// `{uri}local`, or `local` outside a namespace; `None` for an unbound
    /// prefix. Unprefixed attributes are in no namespace.
    fn expand(&self, name: &str, element: bool) -> Option<String> {
        let (prefix, local) = match name.split_once(':') {
            Some((prefix, local)) => (prefix, local),
            None if element => ("", name),
            None => return Some(name.to_owned()),
        };
        match self.lookup(prefix) {
            Some(uri) if !uri.is_empty() => Some(format!("{{{uri}}}{local}")),
            Some(_) | None if prefix.is_empty() => Some(local.to_owned()),
            _ => None,
        }
    }

    /// Everything up to and including the end tag `</name>`.
    fn content(&mut self, element: &mut Element, name: &str, depth: usize) -> Step<()> {
        let mut text = String::new();
        let mut before_child = true;
        loop {
            if self.rest().starts_with("</") {
                let close = self.at;
                self.at += 2;
                let closing = self.name(close)?;
                if closing != name {
                    return Err((Fault::Mismatched, close + 2));
                }
                self.skip_space();
                if !self.eat(">") {
                    return self.missing(close);
                }
                break;
            }
            match self.peek() {
                None => return Err(self.end(Fault::NoElement, self.at)),
                Some('<') => before_child &= !self.markup(element, depth, &mut text)?,
                Some(_) => self.characters(&mut text, before_child)?,
            }
        }
        if before_child && !text.is_empty() {
            element.text = Some(text);
        }
        Ok(())
    }

    /// A child, comment, CDATA section or processing instruction; whether it
    /// was a child. CDATA before the first child is text.
    fn markup(&mut self, element: &mut Element, depth: usize, text: &mut String) -> Step<bool> {
        if self.rest().starts_with("<!--") {
            self.comment()?;
        } else if self.rest().starts_with("<![CDATA[") {
            self.at += 9;
            let Some(end) = self.rest().find("]]>") else {
                return Err(self.end(Fault::UnclosedCdata, self.text.len()));
            };
            self.check_chars(end)?;
            if element.children.is_empty() {
                text.push_str(&normalize_newlines(&self.rest()[..end]));
            }
            self.at += end + 3;
        } else if self.rest().starts_with("<?xml") && self.rest()[5..].starts_with(is_space) {
            return Err((Fault::MisplacedDeclaration, self.at));
        } else if self.rest().starts_with("<?") {
            self.instruction()?;
        } else {
            if element.children.is_empty() && !text.is_empty() {
                element.text = Some(std::mem::take(text));
            }
            let child = self.element(depth + 1)?;
            element.children.push(child);
            return Ok(true);
        }
        Ok(false)
    }

    /// Character data and references up to the next `<`.
    fn characters(&mut self, text: &mut String, keep: bool) -> Step<()> {
        loop {
            match self.peek() {
                None | Some('<') => return Ok(()),
                Some('&') => {
                    let expanded = self.reference()?;
                    if keep {
                        text.push_str(&expanded);
                    }
                }
                Some(']') if self.rest().starts_with("]]>") => {
                    return Err((Fault::InvalidToken, self.at + 2));
                }
                Some('\r') => {
                    self.at += 1;
                    self.eat("\n");
                    if keep {
                        text.push('\n');
                    }
                }
                Some(c) if !is_char(c) => return Err((Fault::InvalidToken, self.at)),
                Some(c) => {
                    self.at += c.len_utf8();
                    if keep {
                        text.push(c);
                    }
                }
            }
        }
    }

    /// `&name;`, `&#N;` or `&#xH;` at the cursor, expanded.
    fn reference(&mut self) -> Step<String> {
        let start = self.at;
        let Some(end) = self.rest().find(';') else {
            return Err(self.end(Fault::Unclosed, start));
        };
        let body = &self.rest()[1..end];
        let expanded = match body {
            "lt" => "<".to_owned(),
            "gt" => ">".to_owned(),
            "amp" => "&".to_owned(),
            "quot" => "\"".to_owned(),
            "apos" => "'".to_owned(),
            _ => match body.strip_prefix('#') {
                Some(number) => character(number)
                    .ok_or((Fault::BadCharacter, start))?
                    .to_string(),
                None if !body.is_empty() && body.chars().all(is_name_char) => {
                    return Err((Fault::UndefinedEntity, start));
                }
                None => return Err((Fault::InvalidToken, start + 1)),
            },
        };
        self.at += end + 1;
        Ok(expanded)
    }
}

/// The character a reference's digits name, if it is an XML character.
fn character(number: &str) -> Option<char> {
    let value = match number.strip_prefix('x') {
        Some(hex) if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
            u32::from_str_radix(hex, 16).ok()?
        }
        Some(_) => return None,
        None if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) => {
            number.parse().ok()?
        }
        None => return None,
    };
    char::from_u32(value).filter(|c| is_char(*c))
}

/// XML's end-of-line handling: `\r\n` and a lone `\r` read as `\n`.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

const fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// XML 1.0's `Char`.
fn is_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | ' '..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..)
}

/// XML 1.0 (fifth edition)'s `NameStartChar`.
fn is_name_start(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z' | '\u{c0}'..='\u{d6}' | '\u{d8}'..='\u{f6}'
        | '\u{f8}'..='\u{2ff}' | '\u{370}'..='\u{37d}' | '\u{37f}'..='\u{1fff}'
        | '\u{200c}'..='\u{200d}' | '\u{2070}'..='\u{218f}' | '\u{2c00}'..='\u{2fef}'
        | '\u{3001}'..='\u{d7ff}' | '\u{f900}'..='\u{fdcf}' | '\u{fdf0}'..='\u{fffd}'
        | '\u{10000}'..='\u{effff}')
}

/// XML 1.0 (fifth edition)'s `NameChar`.
fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{b7}')
        || matches!(c, '\u{300}'..='\u{36f}' | '\u{203f}'..='\u{2040}')
}

#[cfg(test)]
mod tests;
