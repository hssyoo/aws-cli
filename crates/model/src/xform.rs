//! botocore's `xform_name`: operation and member names in camel case to the
//! CLI's hyphenated (or Python's underscored) form.
//!
//! The irregular names are botocore's own `_xform_cache` seed, read from the
//! vendored `awscli/botocore/__init__.py`, never copied here. The payload
//! holds that module only as bytecode, so the source is compiled in from the
//! tree the payload is packed from (`xtask`'s `deps::aws_cli_source`).

/// The vendored botocore package's `__init__.py`.
const BOTOCORE_INIT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../aws-cli/awscli/botocore/__init__.py"
));

/// The `_xform_cache` entries botocore ships: `(name, sep) -> transformed`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpecialCases {
    entries: Vec<(Box<str>, Box<str>, Box<str>)>,
}

/// Why `_xform_cache` could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XformError {
    /// The assignment is missing, or holds something other than
    /// `(string, string): string` items written as plain literals; the
    /// byte offset says where reading stopped.
    Malformed(usize),
}

impl std::fmt::Display for XformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(at) => write!(f, "unreadable _xform_cache at byte {at}"),
        }
    }
}

impl std::error::Error for XformError {}

impl SpecialCases {
    /// The special cases the CLI's botocore ships.
    pub fn botocore() -> Result<Self, XformError> {
        Self::parse(BOTOCORE_INIT)
    }

    /// Reads the `_xform_cache = {...}` assignment from botocore's
    /// `__init__.py` source. Only plain string literals without escapes are
    /// accepted, which is all botocore writes there; anything else is an
    /// error rather than a guess.
    pub fn parse(source: &str) -> Result<Self, XformError> {
        let start = assignment(source).ok_or(XformError::Malformed(0))?;
        let mut tokens = Tokens { source, at: start };
        let mut cases = Self::default();
        loop {
            if tokens.eat('}')? {
                return Ok(cases);
            }
            tokens.expect('(')?;
            let name = tokens.string()?;
            tokens.expect(',')?;
            let sep = tokens.string()?;
            tokens.eat(',')?;
            tokens.expect(')')?;
            tokens.expect(':')?;
            let value = tokens.string()?;
            cases.insert(name, sep, value);
            if !tokens.eat(',')? {
                tokens.expect('}')?;
                return Ok(cases);
            }
        }
    }

    /// Dict assignment: a repeated key keeps its place with the last value.
    fn insert(&mut self, name: &str, sep: &str, value: &str) {
        match self
            .entries
            .iter_mut()
            .find(|(n, s, _)| &**n == name && &**s == sep)
        {
            Some(entry) => entry.2 = value.into(),
            None => self.entries.push((name.into(), sep.into(), value.into())),
        }
    }

    fn get(&self, name: &str, sep: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(n, s, _)| &**n == name && &**s == sep)
            .map(|(_, _, value)| &**value)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The offset just past `_xform_cache = {` at the start of a line.
fn assignment(source: &str) -> Option<usize> {
    const HEAD: &str = "_xform_cache = {";
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        if line.starts_with(HEAD) {
            return Some(offset + HEAD.len());
        }
        offset += line.len();
    }
    None
}

struct Tokens<'a> {
    source: &'a str,
    at: usize,
}

impl<'a> Tokens<'a> {
    /// Skips whitespace and `#` comments.
    fn skip(&mut self) {
        loop {
            let rest = &self.source[self.at..];
            let trimmed = rest.trim_start();
            self.at += rest.len() - trimmed.len();
            if !trimmed.starts_with('#') {
                return;
            }
            self.at += trimmed.find('\n').unwrap_or(trimmed.len());
        }
    }

    fn eat(&mut self, token: char) -> Result<bool, XformError> {
        self.skip();
        if self.at >= self.source.len() {
            return Err(XformError::Malformed(self.at));
        }
        let found = self.source[self.at..].starts_with(token);
        if found {
            self.at += token.len_utf8();
        }
        Ok(found)
    }

    fn expect(&mut self, token: char) -> Result<(), XformError> {
        if self.eat(token)? {
            Ok(())
        } else {
            Err(XformError::Malformed(self.at))
        }
    }

    fn string(&mut self) -> Result<&'a str, XformError> {
        self.skip();
        let rest = &self.source[self.at..];
        let quote = rest
            .chars()
            .next()
            .filter(|quote| matches!(quote, '\'' | '"'))
            .ok_or(XformError::Malformed(self.at))?;
        let body = &rest[1..];
        let end = body.find(quote).ok_or(XformError::Malformed(self.at))?;
        let text = &body[..end];
        if text.contains(['\\', '\n']) {
            return Err(XformError::Malformed(self.at));
        }
        self.at += end + 2;
        Ok(text)
    }
}

/// `botocore.xform_name(name, sep)`: a name already holding `sep` is
/// returned as is; otherwise a special case applies, or the regular rules:
/// a trailing run of capitals plus `s` (`ARNs`) is split off and lowered
/// first, then `(.)([A-Z][a-z]+)` and `([a-z0-9])([A-Z])` get `sep` between
/// their groups, and the result is lowered.
pub fn xform_name(name: &str, sep: &str, special: &SpecialCases) -> String {
    if name.contains(sep) {
        return name.to_owned();
    }
    if let Some(value) = special.get(name, sep) {
        return value.to_owned();
    }
    let chars: Vec<char> = name.chars().collect();
    let chars = match plural_capitals(&chars) {
        Some(matched) => {
            let keep = chars.len().saturating_sub(matched.len());
            let mut joined: Vec<char> = chars[..keep].to_vec();
            joined.extend(sep.chars());
            joined.extend(matched.iter().collect::<String>().to_lowercase().chars());
            joined
        }
        None => chars,
    };
    let first = first_cap(&chars, sep);
    end_cap(&first, sep).to_lowercase()
}

/// `_special_case_transform.search(name)` for `[A-Z]{2,}s$`: the leftmost
/// match, where `$` also matches before a final newline.
fn plural_capitals(chars: &[char]) -> Option<&[char]> {
    let end = match chars {
        [.., 's', '\n'] => chars.len() - 1,
        [.., 's'] => chars.len(),
        _ => return None,
    };
    let s = end - 1;
    let run = chars[..s]
        .iter()
        .rev()
        .take_while(|c| c.is_ascii_uppercase())
        .count();
    (run >= 2).then(|| &chars[s - run..end])
}

/// `_first_cap_regex.sub(r'\1' + sep + r'\2', name)` for `(.)([A-Z][a-z]+)`.
fn first_cap(chars: &[char], sep: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len() + 8);
    let mut index = 0;
    while index < chars.len() {
        let matched = chars[index] != '\n'
            && chars.get(index + 1).is_some_and(char::is_ascii_uppercase)
            && chars.get(index + 2).is_some_and(char::is_ascii_lowercase);
        if !matched {
            out.push(chars[index]);
            index += 1;
            continue;
        }
        let lower = chars[index + 2..]
            .iter()
            .take_while(|c| c.is_ascii_lowercase())
            .count();
        let end = index + 2 + lower;
        out.push(chars[index]);
        out.extend(sep.chars());
        out.extend_from_slice(&chars[index + 1..end]);
        index = end;
    }
    out
}

/// `_end_cap_regex.sub(r'\1' + sep + r'\2', s1)` for `([a-z0-9])([A-Z])`.
fn end_cap(chars: &[char], sep: &str) -> String {
    let mut out = String::with_capacity(chars.len() + 8);
    let mut index = 0;
    while index < chars.len() {
        let current = chars[index];
        let next = chars.get(index + 1);
        out.push(current);
        if (current.is_ascii_lowercase() || current.is_ascii_digit())
            && next.is_some_and(char::is_ascii_uppercase)
        {
            out.push_str(sep);
            out.extend(next);
            index += 2;
        } else {
            index += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests;
