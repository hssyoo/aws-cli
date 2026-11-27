//! `jmespath/lexer.py`: the expression's tokens, positions in code points,
//! and its `LexerError`s.
use crate::error::Error;
use crate::runtime::Runtime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Dot,
    Star,
    Rbracket,
    Comma,
    Colon,
    Current,
    Lparen,
    Rparen,
    Lbrace,
    Rbrace,
    UnquotedIdentifier,
    QuotedIdentifier,
    Flatten,
    Filter,
    Lbracket,
    Literal,
    Or,
    Pipe,
    And,
    Expref,
    Number,
    Lte,
    Lt,
    Gte,
    Gt,
    Ne,
    Not,
    Eq,
    Eof,
}

impl Kind {
    /// The token type's name in `lexer.py`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Dot => "dot",
            Self::Star => "star",
            Self::Rbracket => "rbracket",
            Self::Comma => "comma",
            Self::Colon => "colon",
            Self::Current => "current",
            Self::Lparen => "lparen",
            Self::Rparen => "rparen",
            Self::Lbrace => "lbrace",
            Self::Rbrace => "rbrace",
            Self::UnquotedIdentifier => "unquoted_identifier",
            Self::QuotedIdentifier => "quoted_identifier",
            Self::Flatten => "flatten",
            Self::Filter => "filter",
            Self::Lbracket => "lbracket",
            Self::Literal => "literal",
            Self::Or => "or",
            Self::Pipe => "pipe",
            Self::And => "and",
            Self::Expref => "expref",
            Self::Number => "number",
            Self::Lte => "lte",
            Self::Lt => "lt",
            Self::Gte => "gte",
            Self::Gt => "gt",
            Self::Ne => "ne",
            Self::Not => "not",
            Self::Eq => "eq",
            Self::Eof => "eof",
        }
    }

    /// `Parser.BINDING_POWER`.
    pub(crate) const fn binding_power(self) -> u8 {
        match self {
            Self::Pipe => 1,
            Self::Or => 2,
            Self::And => 3,
            Self::Eq | Self::Gt | Self::Lt | Self::Gte | Self::Lte | Self::Ne => 5,
            Self::Flatten => 9,
            Self::Star => 20,
            Self::Filter => 21,
            Self::Dot => 40,
            Self::Not => 45,
            Self::Lbrace => 50,
            Self::Lbracket => 55,
            Self::Lparen => 60,
            _ => 0,
        }
    }
}

/// A token's value.
#[derive(Debug)]
pub(crate) enum Value<V> {
    /// An identifier's name or a raw string literal, in `View::String`'s
    /// encoding; or the token's own text.
    Text(Vec<u8>),
    /// An integer: saturated to `i64`, and `str()` of the exact one.
    Number(i64, String),
    /// A JSON literal.
    Json(V),
}

#[derive(Debug)]
pub(crate) struct Token<V> {
    pub(crate) kind: Kind,
    pub(crate) value: Value<V>,
    pub(crate) start: usize,
}

struct Lexer<'r, R> {
    runtime: &'r R,
    expression: &'r str,
    chars: Vec<char>,
    position: usize,
    current: Option<char>,
}

/// `Lexer().tokenize(expression)`, run to the end: every token, `eof`
/// last.
pub(crate) fn tokenize<R: Runtime>(
    runtime: &R,
    expression: &str,
) -> Result<Vec<Token<R::Value>>, Error> {
    let chars: Vec<char> = expression.chars().collect();
    let Some(&first) = chars.first() else {
        return Err(Error::empty());
    };
    let mut lexer = Lexer {
        runtime,
        expression,
        chars,
        position: 0,
        current: Some(first),
    };
    let mut tokens = Vec::new();
    while let Some(current) = lexer.current {
        if let Some(token) = lexer.token(current)? {
            tokens.push(token);
        }
    }
    tokens.push(Token {
        kind: Kind::Eof,
        value: Value::Text(Vec::new()),
        start: lexer.chars.len(),
    });
    Ok(tokens)
}

fn text<V>(kind: Kind, text: &str, start: usize) -> Token<V> {
    Token {
        kind,
        value: Value::Text(text.as_bytes().to_vec()),
        start,
    }
}

impl<R: Runtime> Lexer<'_, R> {
    fn next(&mut self) -> Option<char> {
        if self.position + 1 >= self.chars.len() {
            self.current = None;
        } else {
            self.position += 1;
            self.current = Some(self.chars[self.position]);
        }
        self.current
    }

    fn error(&self, position: usize, message: &str) -> Error {
        Error::lexer(self.expression, position, message)
    }

    fn token(&mut self, current: char) -> Result<Option<Token<R::Value>>, Error> {
        let start = self.position;
        let simple = match current {
            '.' => Some(Kind::Dot),
            '*' => Some(Kind::Star),
            ']' => Some(Kind::Rbracket),
            ',' => Some(Kind::Comma),
            ':' => Some(Kind::Colon),
            '@' => Some(Kind::Current),
            '(' => Some(Kind::Lparen),
            ')' => Some(Kind::Rparen),
            '{' => Some(Kind::Lbrace),
            '}' => Some(Kind::Rbrace),
            _ => None,
        };
        if let Some(kind) = simple {
            self.next();
            return Ok(Some(text(kind, current.encode_utf8(&mut [0; 4]), start)));
        }
        Ok(Some(match current {
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut name = String::from(current);
                while let Some(c) = self
                    .next()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    name.push(c);
                }
                text(Kind::UnquotedIdentifier, &name, start)
            }
            ' ' | '\t' | '\n' | '\r' => {
                self.next();
                return Ok(None);
            }
            '[' => match self.next() {
                Some(']') => {
                    self.next();
                    text(Kind::Flatten, "[]", start)
                }
                Some('?') => {
                    self.next();
                    text(Kind::Filter, "[?", start)
                }
                _ => text(Kind::Lbracket, "[", start),
            },
            '\'' => {
                let lexeme = self.until('\'')?.replace("\\'", "'");
                Token {
                    kind: Kind::Literal,
                    value: Value::Text(lexeme.into_bytes()),
                    start,
                }
            }
            '|' => self.or_else('|', Kind::Or, Kind::Pipe),
            '&' => self.or_else('&', Kind::And, Kind::Expref),
            '`' => self.literal()?,
            '0'..='9' | '-' => self.number_token(start)?,
            '"' => self.quoted_identifier()?,
            '<' => self.or_else('=', Kind::Lte, Kind::Lt),
            '>' => self.or_else('=', Kind::Gte, Kind::Gt),
            '!' => self.or_else('=', Kind::Ne, Kind::Not),
            '=' => self.equals()?,
            other => return Err(self.error(start, &format!("Unknown token {other}"))),
        }))
    }

    fn number_token(&mut self, start: usize) -> Result<Token<R::Value>, Error> {
        let mut digits = String::from(self.current.unwrap_or('0'));
        while let Some(c) = self.next().filter(char::is_ascii_digit) {
            digits.push(c);
        }
        if digits == "-" {
            return Err(self.error(start, "Unknown token '-'"));
        }
        let canonical = digits.trim_start_matches('-').trim_start_matches('0');
        let negative = digits.starts_with('-') && !canonical.is_empty();
        let exact = match (negative, canonical) {
            (_, "") => "0".to_owned(),
            (true, digits) => format!("-{digits}"),
            (false, digits) => digits.to_owned(),
        };
        let saturated = exact
            .parse()
            .unwrap_or(if negative { i64::MIN } else { i64::MAX });
        Ok(Token {
            kind: Kind::Number,
            value: Value::Number(saturated, exact),
            start,
        })
    }

    fn equals(&mut self) -> Result<Token<R::Value>, Error> {
        if self.next() == Some('=') {
            let token = text(Kind::Eq, "==", self.position - 1);
            self.next();
            return Ok(token);
        }
        // At the end `next` never moved, so there is nothing to step back.
        let position = if self.current.is_none() {
            self.position
        } else {
            self.position - 1
        };
        Err(self.error(position, "Unknown token '='"))
    }

    /// `_consume_until(delimiter)`: the text to the next unescaped
    /// delimiter, backslashes kept, the delimiter consumed.
    fn until(&mut self, delimiter: char) -> Result<String, Error> {
        let start = self.position;
        let mut text = String::new();
        self.next();
        while self.current != Some(delimiter) {
            if self.current == Some('\\') {
                text.push('\\');
                self.next();
            }
            let Some(c) = self.current else {
                return Err(self.error(start, &format!("Unclosed {delimiter} delimiter")));
            };
            text.push(c);
            self.next();
        }
        self.next();
        Ok(text)
    }

    fn literal(&mut self) -> Result<Token<R::Value>, Error> {
        let start = self.position;
        let lexeme = self.until('`')?.replace("\\`", "`");
        let value = match self.runtime.loads(&lexeme) {
            Ok(value) => value,
            Err(_) => {
                let quoted = format!("\"{}\"", lexeme.trim_start_matches(is_py_space));
                self.runtime
                    .loads(&quoted)
                    .map_err(|_| self.error(start, &format!("Bad token {lexeme}")))?
            }
        };
        Ok(Token {
            kind: Kind::Literal,
            value: Value::Json(value),
            start,
        })
    }

    fn quoted_identifier(&mut self) -> Result<Token<R::Value>, Error> {
        let start = self.position;
        let lexeme = format!("\"{}\"", self.until('"')?);
        let value = self.runtime.loads(&lexeme).map_err(|message| {
            let message = message.split(':').next().unwrap_or_default();
            self.error(start, message)
        })?;
        let crate::runtime::View::String(name) = self.runtime.view(&value) else {
            return Err(self.error(start, "Expecting value"));
        };
        Ok(Token {
            kind: Kind::QuotedIdentifier,
            value: Value::Text(name.to_vec()),
            start,
        })
    }

    fn or_else(&mut self, expected: char, matched: Kind, otherwise: Kind) -> Token<R::Value> {
        let start = self.position;
        let current = self.current.unwrap_or_default();
        if self.next() == Some(expected) {
            self.next();
            return text(matched, &format!("{current}{expected}"), start);
        }
        text(otherwise, current.encode_utf8(&mut [0; 4]), start)
    }
}

/// `str.isspace()` of a character, as `str.lstrip()` strips it.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}
