//! The set's interned strings: names, ids and trait texts.
use std::num::NonZeroU32;

/// A string interned in one [`super::SchemaSetBuilder`] and the set it
/// finishes. An id from another builder is refused only when out of range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StrId(NonZeroU32);

impl StrId {
    /// The id at `index`; `None` for `u32::MAX`, which no pool reaches.
    const fn at(index: u32) -> Option<Self> {
        match NonZeroU32::new(index.wrapping_add(1)) {
            Some(id) => Some(Self(id)),
            None => None,
        }
    }

    /// Its index in the pool. It is stored one up, so an `Option<StrId>`,
    /// as the trait slots hold them, takes four bytes.
    pub(super) const fn index(self) -> usize {
        (self.0.get() - 1) as usize
    }
}

/// Every interned string, end to end, with each one's span.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct StrPool {
    text: Box<str>,
    spans: Box<[(u32, u32)]>,
}

impl StrPool {
    pub(super) fn get(&self, id: StrId) -> &str {
        let (start, end) = self.spans[id.index()];
        &self.text[start as usize..end as usize]
    }
}

/// A text's 32-bit hash (Fx's multiply-rotate over 8-byte words, then the
/// length): the interner's slot and first comparison. SipHash's cost
/// dominated a large model's build.
pub(super) fn hash(text: &str) -> u32 {
    let add = |hash: u64, word: u64| (hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    let mut chunks = text.as_bytes().chunks_exact(8);
    let mut hash = 0;
    for chunk in &mut chunks {
        let mut word = [0; 8];
        word.copy_from_slice(chunk);
        hash = add(hash, u64::from_le_bytes(word));
    }
    let mut word = [0; 8];
    word[..chunks.remainder().len()].copy_from_slice(chunks.remainder());
    hash = add(add(hash, u64::from_le_bytes(word)), text.len() as u64);
    (hash >> 32) as u32 ^ hash as u32
}

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

/// A table slot: an id with its text's hash; [`EMPTY`] is free.
#[derive(Clone, Copy, Debug)]
struct Slot {
    hash: u32,
    id: u32,
}

/// No id in a [`Slot`].
const EMPTY: u32 = u32::MAX;

const FREE: Slot = Slot { hash: 0, id: EMPTY };

/// The pool while a builder fills it: the texts end to end, and an
/// open-addressed table of ids by each text's hash, so interning allocates
/// no key of its own.
#[derive(Debug, Default)]
pub(super) struct Interner {
    text: String,
    spans: Vec<(u32, u32)>,
    /// A power-of-two count of slots, at most half full.
    table: Vec<Slot>,
    /// How many ids the table holds.
    indexed: usize,
}

impl Interner {
    /// Room for `count` strings of `bytes` in all, `indexed` of them
    /// interned, ahead of time.
    pub(super) fn reserve(&mut self, count: usize, indexed: usize, bytes: usize) {
        self.text.reserve(bytes);
        self.spans.reserve(count);
        let wanted = (self.indexed + indexed)
            .saturating_mul(2)
            .next_power_of_two();
        if wanted > self.table.len() {
            self.rehash(wanted);
        }
    }

    fn rehash(&mut self, slots: usize) {
        let old = std::mem::replace(&mut self.table, vec![FREE; slots]);
        let mask = slots - 1;
        for entry in old.into_iter().filter(|entry| entry.id != EMPTY) {
            let mut slot = entry.hash as usize & mask;
            while self.table[slot].id != EMPTY {
                slot = (slot + 1) & mask;
            }
            self.table[slot] = entry;
        }
    }

    /// Whether `id`'s text is `text`.
    pub(super) fn is(&self, id: StrId, text: &str) -> bool {
        self.is_at(id.index(), text)
    }

    fn is_at(&self, index: usize, text: &str) -> bool {
        let (start, end) = self.spans[index];
        (end - start) as usize == text.len()
            && self.text.as_bytes().get(start as usize..end as usize) == Some(text.as_bytes())
    }

    /// The table slot holding `text`, or the free slot it would take.
    fn slot(&self, text: &str, hash: u32) -> usize {
        let mask = self.table.len() - 1;
        let mut slot = hash as usize & mask;
        loop {
            let entry = self.table[slot];
            if entry.id == EMPTY || (entry.hash == hash && self.is_at(entry.id as usize, text)) {
                return slot;
            }
            slot = (slot + 1) & mask;
        }
    }

    /// The id of `text` if it was interned.
    pub(super) fn find(&self, text: &str) -> Option<StrId> {
        if self.table.is_empty() {
            return None;
        }
        let id = self.table[self.slot(text, hash(text))].id;
        StrId::at(id)
    }

    /// The id of `text`, interned on first use; `None` past `u32` offsets.
    pub(super) fn intern(&mut self, text: &str) -> Option<StrId> {
        if self.table.len() < (self.indexed + 1) * 2 {
            self.rehash((self.table.len() * 2).max(64));
        }
        let hash = hash(text);
        let slot = self.slot(text, hash);
        if self.table[slot].id != EMPTY {
            return StrId::at(self.table[slot].id);
        }
        let id = self.push(text)?;
        self.table[slot] = Slot {
            hash,
            id: u32::try_from(id.index()).ok()?,
        };
        self.indexed += 1;
        Some(id)
    }

    fn push(&mut self, text: &str) -> Option<StrId> {
        let start = u32::try_from(self.text.len()).ok()?;
        let end = u32::try_from(self.text.len() + text.len()).ok()?;
        let id = u32::try_from(self.spans.len())
            .ok()
            .filter(|id| *id != EMPTY)?;
        self.text.push_str(text);
        self.spans.push((start, end));
        StrId::at(id)
    }

    /// Stores `container`'s text, `$` and `member` as a member's id, and
    /// gives that id and one for the member's name, its end. Neither is
    /// one [`Self::find`] or [`Self::intern`] returns: a member's id is
    /// unique, and both are only read.
    pub(super) fn push_member(&mut self, container: StrId, member: &str) -> Option<(StrId, StrId)> {
        let (from, to) = self.spans[container.index()];
        let start = u32::try_from(self.text.len()).ok()?;
        let len = (to - from) as usize + 1 + member.len();
        let end = u32::try_from(self.text.len() + len).ok()?;
        let name_at = end - u32::try_from(member.len()).ok()?;
        let id = u32::try_from(self.spans.len())
            .ok()
            .filter(|id| *id < EMPTY - 1)?;
        self.text.extend_from_within(from as usize..to as usize);
        self.text.push('$');
        self.text.push_str(member);
        self.spans.push((start, end));
        self.spans.push((name_at, end));
        Some((StrId::at(id)?, StrId::at(id + 1)?))
    }

    pub(super) fn get(&self, id: StrId) -> &str {
        let (start, end) = self.spans[id.index()];
        &self.text[start as usize..end as usize]
    }

    pub(super) fn contains(&self, id: StrId) -> bool {
        id.index() < self.spans.len()
    }

    pub(super) fn finish(self) -> StrPool {
        StrPool {
            text: self.text.into(),
            spans: self.spans.into(),
        }
    }
}

#[cfg(test)]
mod tests;
