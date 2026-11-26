//! Shape ids: `namespace#Name` and `namespace#Name$member`.
use std::fmt;

/// A shape id borrowed from its text: `namespace#Name` or
/// `namespace#Name$member`, as smithy-rs's `ShapeId<'a>`.
///
/// The check is structural, not Smithy's identifier grammar: each part is
/// non-empty and holds no `#` or `$`. botocore's own shape and member names
/// are not all Smithy identifiers, and the C2J namespace is
/// `c2j.<service-dir>`, whose directory names hold hyphens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShapeId<'s> {
    fqn: &'s str,
    sharp_at: u16,
    dollar_at: Option<u16>,
}

/// Why a text is not a shape id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeIdError {
    /// No `#`, or a `#` or `$` in the wrong place or twice.
    Malformed(Box<str>),
    /// An empty namespace, name or member.
    EmptyPart(Box<str>),
    /// Longer than the 64 KiB a set's offsets can hold.
    TooLong,
}

impl fmt::Display for ShapeIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(text) => write!(f, "malformed shape id: {text}"),
            Self::EmptyPart(text) => write!(f, "shape id with an empty part: {text}"),
            Self::TooLong => f.write_str("shape id too long"),
        }
    }
}

impl std::error::Error for ShapeIdError {}

impl<'s> ShapeId<'s> {
    /// Parses `namespace#Name` or `namespace#Name$member`.
    pub fn parse(fqn: &'s str) -> Result<Self, ShapeIdError> {
        let (sharp_at, dollar_at) = offsets(fqn)?;
        Ok(Self {
            fqn,
            sharp_at,
            dollar_at,
        })
    }

    /// The parts' offsets, checked: what a set stores beside the text.
    pub(super) fn from_parts(fqn: &'s str, sharp_at: u16, dollar_at: Option<u16>) -> Self {
        Self {
            fqn,
            sharp_at,
            dollar_at,
        }
    }

    pub const fn as_str(self) -> &'s str {
        self.fqn
    }

    pub fn namespace(self) -> &'s str {
        &self.fqn[..usize::from(self.sharp_at)]
    }

    /// The shape's own name: botocore's short shape name.
    pub fn name(self) -> &'s str {
        let end = self.dollar_at.map_or(self.fqn.len(), usize::from);
        &self.fqn[usize::from(self.sharp_at) + 1..end]
    }

    pub fn member(self) -> Option<&'s str> {
        self.dollar_at.map(|at| &self.fqn[usize::from(at) + 1..])
    }

    /// The id without its member: the containing shape's.
    pub fn root(self) -> Self {
        let end = self.dollar_at.map_or(self.fqn.len(), usize::from);
        Self::from_parts(&self.fqn[..end], self.sharp_at, None)
    }
}

impl fmt::Display for ShapeId<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.fqn)
    }
}

/// The offsets of `#` and `$` in a valid id.
pub(super) fn offsets(fqn: &str) -> Result<(u16, Option<u16>), ShapeIdError> {
    let malformed = || ShapeIdError::Malformed(fqn.into());
    let sharp = fqn.find('#').ok_or_else(malformed)?;
    let rest = &fqn[sharp + 1..];
    let dollar = rest.find('$').map(|at| sharp + 1 + at);
    let name_end = dollar.unwrap_or(fqn.len());
    let member = dollar.map(|at| &fqn[at + 1..]);
    if rest.contains('#') || member.is_some_and(|text| text.contains('$')) {
        return Err(malformed());
    }
    let parts = [&fqn[..sharp], &fqn[sharp + 1..name_end]];
    if parts.iter().chain(&member).any(|part| part.is_empty()) {
        return Err(ShapeIdError::EmptyPart(fqn.into()));
    }
    if fqn[..sharp].contains('$') {
        return Err(malformed());
    }
    let short = |at: usize| u16::try_from(at).map_err(|_| ShapeIdError::TooLong);
    if fqn.len() > usize::from(u16::MAX) {
        return Err(ShapeIdError::TooLong);
    }
    Ok((short(sharp)?, dollar.map(short).transpose()?))
}

#[cfg(test)]
mod tests;
