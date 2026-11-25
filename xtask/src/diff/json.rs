//! A lossless reading of JSON text for comparison: every member in its
//! order, every scalar as its own text, and the whitespace around each
//! token. Two texts that read the same have the same bytes, so the
//! comparison can name key order, number or escape spelling and layout
//! apart from values (`compare`).
//!
//! The reading is lenient about scalars (any run of letters, digits, `.`,
//! `+` and `-`, so Python's `NaN` and `Infinity` read too) and strict about
//! structure; text that is not one JSON value reads as none.

/// The deepest nesting read; deeper text reads as none.
const DEPTH_MAX: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document<'a> {
    pub root: Node<'a>,
    pub before: &'a str,
    pub after: &'a str,
}

/// A value: its whole text, and its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node<'a> {
    pub text: &'a str,
    pub kind: Kind<'a>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind<'a> {
    /// The members, and the whitespace before `}`.
    Object(Vec<Member<'a>>, &'a str),
    /// The elements, and the whitespace before `]`.
    Array(Vec<Element<'a>>, &'a str),
    /// A string, number or literal, whose text is all of it.
    Scalar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member<'a> {
    /// The key's text, quotes and escapes included.
    pub key: &'a str,
    pub value: Node<'a>,
    /// The whitespace before the key, before `:` and before the value.
    pub layout: [&'a str; 3],
    /// The whitespace before the `,` after it, when one follows.
    pub before_comma: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element<'a> {
    pub value: Node<'a>,
    pub before: &'a str,
    pub before_comma: Option<&'a str>,
}

/// `text` as one JSON value, or none.
pub fn read(text: &str) -> Option<Document<'_>> {
    let mut reader = Reader {
        text,
        at: 0,
        depth: 0,
    };
    let before = reader.space();
    let root = reader.value()?;
    let after = reader.space();
    (reader.at == text.len()).then_some(Document {
        root,
        before,
        after,
    })
}

#[derive(Debug)]
struct Reader<'a> {
    text: &'a str,
    at: usize,
    depth: usize,
}

impl<'a> Reader<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        (self.peek() == Some(byte)).then(|| self.at += 1)
    }

    fn space(&mut self) -> &'a str {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.at += 1;
        }
        &self.text[start..self.at]
    }

    fn value(&mut self) -> Option<Node<'a>> {
        let start = self.at;
        let kind = match self.peek()? {
            b'{' => self.nested(Self::object)?,
            b'[' => self.nested(Self::array)?,
            b'"' => self.string().map(|_| Kind::Scalar)?,
            _ => self.scalar()?,
        };
        Some(Node {
            text: &self.text[start..self.at],
            kind,
        })
    }

    fn nested(&mut self, read: fn(&mut Self) -> Option<Kind<'a>>) -> Option<Kind<'a>> {
        self.depth += 1;
        if self.depth > DEPTH_MAX {
            return None;
        }
        let kind = read(self);
        self.depth -= 1;
        kind
    }

    fn object(&mut self) -> Option<Kind<'a>> {
        self.eat(b'{')?;
        let mut members = Vec::new();
        let mut before_key = self.space();
        if self.eat(b'}').is_some() {
            return Some(Kind::Object(members, before_key));
        }
        loop {
            let key = self.string()?;
            let before_colon = self.space();
            self.eat(b':')?;
            let before_value = self.space();
            let value = self.value()?;
            let after = self.space();
            let more = self.eat(b',').is_some();
            members.push(Member {
                key,
                value,
                layout: [before_key, before_colon, before_value],
                before_comma: more.then_some(after),
            });
            if !more {
                self.eat(b'}')?;
                return Some(Kind::Object(members, after));
            }
            before_key = self.space();
        }
    }

    fn array(&mut self) -> Option<Kind<'a>> {
        self.eat(b'[')?;
        let mut elements = Vec::new();
        let mut before = self.space();
        if self.eat(b']').is_some() {
            return Some(Kind::Array(elements, before));
        }
        loop {
            let value = self.value()?;
            let after = self.space();
            let more = self.eat(b',').is_some();
            elements.push(Element {
                value,
                before,
                before_comma: more.then_some(after),
            });
            if !more {
                self.eat(b']')?;
                return Some(Kind::Array(elements, after));
            }
            before = self.space();
        }
    }

    /// A string's text, quotes included.
    fn string(&mut self) -> Option<&'a str> {
        let start = self.at;
        self.eat(b'"')?;
        let mut escaped = false;
        for (offset, byte) in self.text.as_bytes()[self.at..].iter().enumerate() {
            match (escaped, byte) {
                (false, b'"') => {
                    self.at += offset + 1;
                    return Some(&self.text[start..self.at]);
                }
                (false, b'\\') => escaped = true,
                _ => escaped = false,
            }
        }
        None
    }

    fn scalar(&mut self) -> Option<Kind<'a>> {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
        {
            self.at += 1;
        }
        (self.at > start).then_some(Kind::Scalar)
    }
}

#[cfg(test)]
mod tests;
