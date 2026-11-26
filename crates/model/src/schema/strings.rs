//! The set's interned strings: names, ids and trait texts.
use std::collections::HashMap;

/// A string interned in one [`super::SchemaSetBuilder`] and the set it
/// finishes. An id from another builder is refused only when out of range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StrId(u32);

/// Every interned string, end to end, with each one's span.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct StrPool {
    text: Box<str>,
    spans: Box<[(u32, u32)]>,
}

impl StrPool {
    pub(super) fn get(&self, id: StrId) -> &str {
        let (start, end) = self.spans[id.0 as usize];
        &self.text[start as usize..end as usize]
    }
}

/// The pool while a builder fills it.
#[derive(Debug, Default)]
pub(super) struct Interner {
    text: String,
    spans: Vec<(u32, u32)>,
    index: HashMap<Box<str>, StrId>,
}

impl Interner {
    /// The id of `text`, interned on first use; `None` past `u32` offsets.
    pub(super) fn intern(&mut self, text: &str) -> Option<StrId> {
        if let Some(id) = self.index.get(text) {
            return Some(*id);
        }
        let start = u32::try_from(self.text.len()).ok()?;
        let end = u32::try_from(self.text.len() + text.len()).ok()?;
        let id = StrId(u32::try_from(self.spans.len()).ok()?);
        self.text.push_str(text);
        self.spans.push((start, end));
        self.index.insert(text.into(), id);
        Some(id)
    }

    pub(super) fn get(&self, id: StrId) -> &str {
        let (start, end) = self.spans[id.0 as usize];
        &self.text[start as usize..end as usize]
    }

    pub(super) fn contains(&self, id: StrId) -> bool {
        (id.0 as usize) < self.spans.len()
    }

    pub(super) fn finish(self) -> StrPool {
        StrPool {
            text: self.text.into(),
            spans: self.spans.into(),
        }
    }
}
