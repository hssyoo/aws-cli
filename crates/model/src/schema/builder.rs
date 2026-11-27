//! Building a [`SchemaSet`]: a front-end declares shapes, members, traits
//! and operations through [`DraftId`] handles, and [`SchemaSetBuilder::finish`]
//! lays the arena out (named shapes first) and checks it.
//!
//! The builder takes resolved trait sets, never deltas: each front-end
//! owns its member-over-target merge (C2J merges keys before translating,
//! Smithy uses [`KnownTraits::overlaid`] and [`ColdTraits::overlaid`]).
use super::SchemaSet;
use super::cold::{ColdEntries, ColdTraits};
use super::id::{ShapeIdError, offsets};
use super::known::KnownTraits;
use super::operation::{OperationData, OperationIndex, ServiceData};
use super::strings::{Interner, StrId, hash};
use super::types::{PRELUDE, PRELUDE_NAMESPACE, ShapeType};
use super::uninterpreted::{Site, Uninterpreted};
use std::collections::HashSet;

mod layout;

/// A shape, member or overlay while its set is built. Only its builder
/// reads it: [`SchemaSetBuilder::finish`] renumbers everything, so a draft
/// id is never a [`super::SchemaId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DraftId(u32);

impl DraftId {
    const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A reference's traits: the target's own, or a resolved (merged) set.
#[derive(Clone, Debug, PartialEq)]
pub enum MemberTraits {
    Inherit,
    Resolved(Box<KnownTraits>, ColdTraits),
}

impl MemberTraits {
    pub fn resolved(known: KnownTraits, cold: ColdTraits) -> Self {
        Self::Resolved(Box::new(known), cold)
    }
}

/// An operation's input, output and errors, as drafts. An input or output
/// that targets the prelude's `Unit` is none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperationShapes {
    pub input: Option<DraftId>,
    pub output: Option<DraftId>,
    pub errors: Vec<DraftId>,
}

/// Why a builder refuses a call or its set.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BuildError {
    ShapeId(ShapeIdError),
    /// A shape declared twice.
    Duplicate(Box<str>),
    /// A referenced shape never declared.
    NoShapeFound(Box<str>),
    /// An `Unknown` shape with no type text, a list without its member, a
    /// map without its key and value, or a member id where a shape is due.
    InvalidShape(Box<str>),
    /// A member added to a shape whose type holds none of that kind.
    InvalidParent(Box<str>),
    /// A reference to a member or overlay, or traits set on the prelude.
    InvalidTarget(Box<str>),
    DuplicateMember(Box<str>),
    /// Two named shapes with one short name.
    NameClash(Box<str>),
    /// An unknown trait through `insert`, or a malformed one.
    UnknownTrait(Box<str>),
    /// A known trait through `insert_unknown`.
    KnownTrait(Box<str>),
    /// A slot-stored trait offered to the cold map.
    SlotTrait(Box<str>),
    DuplicateTrait(Box<str>),
    DuplicateOperation(Box<str>),
    /// An uninterpreted key at an operation or shape the set does not have.
    UnknownSite(Box<str>),
    /// A string or draft id this builder did not hand out.
    Foreign,
    TooLarge,
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (what, subject) = match self {
            Self::ShapeId(error) => return error.fmt(f),
            Self::Foreign => return f.write_str("an id from another schema builder"),
            Self::TooLarge => return f.write_str("the model has too many shapes"),
            Self::Duplicate(s) => ("duplicate shape", s),
            Self::NoShapeFound(s) => ("Shape not found", s),
            Self::InvalidShape(s) => ("invalid shape", s),
            Self::InvalidParent(s) => ("no such members in", s),
            Self::InvalidTarget(s) => ("not a shape to target", s),
            Self::DuplicateMember(s) => ("duplicate member", s),
            Self::NameClash(s) => ("two shapes named", s),
            Self::UnknownTrait(s) => ("unknown trait", s),
            Self::KnownTrait(s) => ("known trait recorded as unknown", s),
            Self::SlotTrait(s) => ("slot trait in the cold map", s),
            Self::DuplicateTrait(s) => ("duplicate trait", s),
            Self::DuplicateOperation(s) => ("duplicate operation", s),
            Self::UnknownSite(s) => ("uninterpreted key at no such site", s),
        };
        write!(f, "{what}: {subject}")
    }
}

impl std::error::Error for BuildError {}

impl From<ShapeIdError> for BuildError {
    fn from(error: ShapeIdError) -> Self {
        Self::ShapeId(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DraftRole {
    /// Referenced, not yet declared.
    Placeholder,
    Named,
    Member {
        container: DraftId,
        name: StrId,
        target: DraftId,
    },
    Overlay {
        target: DraftId,
    },
    Prelude,
}

#[derive(Clone, Debug, PartialEq)]
enum DraftChildren {
    None,
    Members(Vec<DraftId>),
    List(Option<DraftId>),
    Map(Option<(DraftId, DraftId)>),
}

/// A draft's traits: its own (an index into
/// [`SchemaSetBuilder::known_sets`] and the cold entries), or its target's.
#[derive(Clone, Debug, PartialEq)]
enum DraftTraits {
    Own(KnownSet, ColdEntries),
    Inherit,
}

/// An index into [`SchemaSetBuilder::known_sets`]; [`EMPTY_SET`] is the
/// empty set every draft starts with.
type KnownSet = u32;
const EMPTY_SET: KnownSet = 0;

#[derive(Clone, Debug)]
struct Draft {
    id: StrId,
    sharp_at: u16,
    dollar_at: Option<u16>,
    shape_type: ShapeType,
    type_name: Option<StrId>,
    role: DraftRole,
    children: DraftChildren,
    traits: DraftTraits,
}

/// Builds one [`SchemaSet`]; the prelude is declared from the start.
#[derive(Debug)]
pub struct SchemaSetBuilder {
    namespace: Box<str>,
    strings: Interner,
    drafts: Vec<Draft>,
    /// Each named, placeholder or prelude draft by its interned id.
    /// Indexed by the id's [`StrId`]; `None` past the end or for another text.
    by_fqn: Vec<Option<DraftId>>,
    /// Each draft's own known traits, by [`KnownSet`]; each set but the
    /// shared empty one, [`EMPTY_SET`], has one draft.
    known_sets: Vec<KnownTraits>,
    /// Named shapes in declaration order.
    order: Vec<DraftId>,
    overlays: Vec<DraftId>,
    operations: Vec<(OperationData, Box<str>, u16, OperationShapes)>,
    operation_names: HashSet<Box<str>>,
    /// Each member of a structure or union with at least [`SCAN_MEMBERS`]
    /// by its container and its name's hash.
    member_names: HashSet<(DraftId, u32)>,
    /// A shape id while it is built.
    scratch: String,
    service: ServiceData,
    uninterpreted: Vec<Uninterpreted>,
}

/// A structure with fewer members than this is checked for a duplicate
/// name by a scan, not through [`SchemaSetBuilder::member_names`].
const SCAN_MEMBERS: usize = 16;

const fn own_traits() -> DraftTraits {
    DraftTraits::Own(EMPTY_SET, Vec::new())
}

fn children_for(shape_type: ShapeType) -> DraftChildren {
    match shape_type {
        ShapeType::List => DraftChildren::List(None),
        ShapeType::Map => DraftChildren::Map(None),
        ty if ty.is_aggregate() => DraftChildren::Members(Vec::new()),
        _ => DraftChildren::None,
    }
}

impl SchemaSetBuilder {
    /// A builder whose short names live in `namespace` (C2J:
    /// `c2j.<service-dir>`).
    pub fn new(namespace: &str) -> Result<Self, BuildError> {
        let mut builder = Self {
            namespace: namespace.into(),
            strings: Interner::default(),
            drafts: Vec::new(),
            by_fqn: Vec::new(),
            known_sets: vec![KnownTraits::default()],
            order: Vec::new(),
            overlays: Vec::new(),
            operations: Vec::new(),
            operation_names: HashSet::new(),
            member_names: HashSet::new(),
            scratch: String::new(),
            service: ServiceData::default(),
            uninterpreted: Vec::new(),
        };
        for (name, shape_type) in PRELUDE {
            let fqn = format!("{PRELUDE_NAMESPACE}#{name}");
            let type_name = builder.default_type_name(&fqn, shape_type, None)?;
            let draft = builder.push(&fqn, shape_type, Some(type_name), DraftRole::Prelude)?;
            builder.index_shape(draft);
        }
        Ok(builder)
    }

    /// Room ahead of time for a model of about `shapes` named shapes, with
    /// about three members, list members or map entries and three distinct
    /// texts each, and `operations` operations: a hint, which changes no
    /// result.
    pub fn reserve(&mut self, shapes: usize, operations: usize) {
        self.operations.reserve(operations);
        self.operation_names.reserve(operations);
        let drafts = shapes.saturating_mul(4);
        self.drafts.reserve(drafts);
        self.known_sets.reserve(drafts);
        let texts = shapes.saturating_mul(3);
        self.strings
            .reserve(drafts, texts, drafts.saturating_mul(48));
    }

    /// Interns a text for a [`KnownTraits`] slot.
    pub fn intern(&mut self, text: &str) -> Result<StrId, BuildError> {
        self.strings.intern(text).ok_or(BuildError::TooLarge)
    }

    fn push(
        &mut self,
        fqn: &str,
        shape_type: ShapeType,
        type_name: Option<StrId>,
        role: DraftRole,
    ) -> Result<DraftId, BuildError> {
        let at = offsets(fqn)?;
        let id = self.intern(fqn)?;
        self.push_with((id, at), shape_type, type_name, role, own_traits())
    }

    /// Pushes a draft of the interned id `id`, whose `#` and `$` offsets
    /// are `at`.
    fn push_with(
        &mut self,
        (id, (sharp_at, dollar_at)): (StrId, (u16, Option<u16>)),
        shape_type: ShapeType,
        type_name: Option<StrId>,
        role: DraftRole,
        traits: DraftTraits,
    ) -> Result<DraftId, BuildError> {
        let draft = Draft {
            id,
            sharp_at,
            dollar_at,
            shape_type,
            type_name,
            role,
            children: children_for(shape_type),
            traits,
        };
        let id = DraftId(u32::try_from(self.drafts.len()).map_err(|_| BuildError::TooLarge)?);
        self.drafts.push(draft);
        Ok(id)
    }

    fn default_type_name(
        &mut self,
        fqn: &str,
        shape_type: ShapeType,
        given: Option<&str>,
    ) -> Result<StrId, BuildError> {
        let text = given.or_else(|| shape_type.default_type_name());
        let text = text.ok_or_else(|| BuildError::InvalidShape(fqn.into()))?;
        self.intern(text)
    }

    /// Runs `f` on `name`'s full id, built in the scratch buffer.
    fn with_qualified<T>(&mut self, name: &str, f: impl FnOnce(&mut Self, &str) -> T) -> T {
        let mut fqn = std::mem::take(&mut self.scratch);
        fqn.clear();
        fqn.push_str(&self.namespace);
        fqn.push('#');
        fqn.push_str(name);
        let result = f(self, &fqn);
        self.scratch = fqn;
        result
    }

    fn draft(&self, id: DraftId) -> Result<&Draft, BuildError> {
        self.drafts.get(id.index()).ok_or(BuildError::Foreign)
    }

    fn fqn(&self, id: DraftId) -> Box<str> {
        self.drafts
            .get(id.index())
            .map_or_else(|| "?".into(), |draft| self.strings.get(draft.id).into())
    }

    /// Declares a named shape in the builder's namespace, in document
    /// order. `type_name` is botocore's `type` text; when absent it is
    /// derived from the type, which an `Unknown` shape cannot do.
    pub fn declare(
        &mut self,
        name: &str,
        shape_type: ShapeType,
        type_name: Option<&str>,
    ) -> Result<DraftId, BuildError> {
        self.with_qualified(name, |builder, fqn| {
            builder.declare_id(fqn, shape_type, type_name)
        })
    }

    /// Declares a named shape by its full id.
    pub fn declare_id(
        &mut self,
        fqn: &str,
        shape_type: ShapeType,
        type_name: Option<&str>,
    ) -> Result<DraftId, BuildError> {
        let at = offsets(fqn)?;
        if at.1.is_some() {
            return Err(BuildError::InvalidShape(fqn.into()));
        }
        let type_name = self.default_type_name(fqn, shape_type, type_name)?;
        // The id is interned where a new draft would intern it, so one
        // lookup serves both.
        let interned = self.intern(fqn)?;
        let id = match self.by_fqn.get(interned.index()).copied().flatten() {
            Some(id) if self.drafts[id.index()].role == DraftRole::Placeholder => id,
            Some(_) => return Err(BuildError::Duplicate(fqn.into())),
            None => self.push_shape((interned, at), shape_type)?,
        };
        let draft = &mut self.drafts[id.index()];
        draft.shape_type = shape_type;
        draft.type_name = Some(type_name);
        draft.role = DraftRole::Named;
        draft.children = children_for(shape_type);
        self.order.push(id);
        Ok(id)
    }

    /// A shape in the builder's namespace, declared or not yet: one never
    /// declared fails [`Self::finish`] with `NoShapeFound`.
    pub fn reference(&mut self, name: &str) -> Result<DraftId, BuildError> {
        self.with_qualified(name, Self::reference_id)
    }

    pub fn reference_id(&mut self, fqn: &str) -> Result<DraftId, BuildError> {
        if let Some(id) = self.lookup(fqn) {
            return Ok(id);
        }
        let at = offsets(fqn)?;
        if at.1.is_some() {
            return Err(BuildError::InvalidShape(fqn.into()));
        }
        let interned = self.intern(fqn)?;
        self.push_shape((interned, at), ShapeType::Unknown)
    }

    /// The named, placeholder or prelude draft with this full id.
    fn lookup(&self, fqn: &str) -> Option<DraftId> {
        let id = self.strings.find(fqn)?;
        self.by_fqn.get(id.index()).copied().flatten()
    }

    /// Makes a shape draft findable by [`Self::lookup`].
    fn index_shape(&mut self, draft: DraftId) {
        let id = self.drafts[draft.index()].id.index();
        if self.by_fqn.len() <= id {
            self.by_fqn.resize(id + 1, None);
        }
        self.by_fqn[id] = Some(draft);
    }

    /// A placeholder for a shape id, findable by [`Self::lookup`].
    fn push_shape(
        &mut self,
        fqn: (StrId, (u16, Option<u16>)),
        shape_type: ShapeType,
    ) -> Result<DraftId, BuildError> {
        let traits = own_traits();
        let id = self.push_with(fqn, shape_type, None, DraftRole::Placeholder, traits)?;
        self.index_shape(id);
        Ok(id)
    }

    /// A new known set's index.
    fn known_set(&mut self, known: KnownTraits) -> Result<KnownSet, BuildError> {
        if known == self.known_sets[EMPTY_SET as usize] {
            return Ok(EMPTY_SET);
        }
        let set = u32::try_from(self.known_sets.len()).map_err(|_| BuildError::TooLarge)?;
        self.known_sets.push(known);
        Ok(set)
    }

    /// A prelude shape by its [`PRELUDE`] name.
    pub fn prelude(&self, name: &str) -> Option<DraftId> {
        self.lookup(&format!("{PRELUDE_NAMESPACE}#{name}"))
    }

    /// Records the cold map's unknown traits as uninterpreted keys.
    fn take_cold(&mut self, cold: ColdTraits) -> ColdEntries {
        let (entries, unknown) = cold.into_parts();
        let records = unknown
            .into_iter()
            .map(|(site, key)| Uninterpreted { site, key });
        self.uninterpreted.extend(records);
        entries
    }

    fn resolved(&mut self, traits: MemberTraits) -> Result<DraftTraits, BuildError> {
        Ok(match traits {
            MemberTraits::Inherit => DraftTraits::Inherit,
            MemberTraits::Resolved(known, cold) => {
                let set = self.known_set(*known)?;
                DraftTraits::Own(set, self.take_cold(cold))
            }
        })
    }

    /// Sets a named shape's own traits.
    pub fn set_traits(
        &mut self,
        id: DraftId,
        known: KnownTraits,
        cold: ColdTraits,
    ) -> Result<(), BuildError> {
        if !matches!(
            self.draft(id)?.role,
            DraftRole::Named | DraftRole::Placeholder
        ) {
            return Err(BuildError::InvalidTarget(self.fqn(id)));
        }
        let entries = self.take_cold(cold);
        let set = self.known_set(known)?;
        self.drafts[id.index()].traits = DraftTraits::Own(set, entries);
        Ok(())
    }

    /// A reference target: a named, placeholder or prelude shape.
    fn check_target(&self, target: DraftId) -> Result<(), BuildError> {
        match self.draft(target)?.role {
            DraftRole::Named | DraftRole::Placeholder | DraftRole::Prelude => Ok(()),
            DraftRole::Member { .. } | DraftRole::Overlay { .. } => {
                Err(BuildError::InvalidTarget(self.fqn(target)))
            }
        }
    }

    fn new_member(
        &mut self,
        container: DraftId,
        name: &str,
        target: DraftId,
        traits: MemberTraits,
    ) -> Result<DraftId, BuildError> {
        self.check_target(target)?;
        let parent = self.draft(container)?;
        let (parent_id, parent_sharp) = (parent.id, parent.sharp_at);
        let traits = self.resolved(traits)?;
        // A plain name (not empty, no `#` or `$`) after a named shape's id
        // makes an id whose offsets are the shape's and its end; any other
        // fails as `offsets` fails it.
        let parent_len = self.strings.get(parent_id).len();
        let plain = !name.is_empty() && !name.bytes().any(|byte| byte == b'#' || byte == b'$');
        let dollar_at = match u16::try_from(parent_len) {
            Ok(at) if plain && parent_len + 1 + name.len() <= usize::from(u16::MAX) => at,
            _ => {
                let fqn = format!("{}${name}", self.strings.get(parent_id));
                return Err(offsets(&fqn).err().unwrap_or(ShapeIdError::TooLong).into());
            }
        };
        let member = DraftId(u32::try_from(self.drafts.len()).map_err(|_| BuildError::TooLarge)?);
        let (id, name) = self
            .strings
            .push_member(parent_id, name)
            .ok_or(BuildError::TooLarge)?;
        self.drafts.push(Draft {
            id,
            sharp_at: parent_sharp,
            dollar_at: Some(dollar_at),
            shape_type: ShapeType::Unknown,
            type_name: None,
            role: DraftRole::Member {
                container,
                name,
                target,
            },
            children: DraftChildren::None,
            traits,
        });
        Ok(member)
    }

    /// The draft of a named shape (not a placeholder) to add children to.
    fn parent(&self, parent: DraftId) -> Result<&Draft, BuildError> {
        let draft = self.draft(parent)?;
        if draft.role != DraftRole::Named {
            return Err(BuildError::InvalidParent(self.fqn(parent)));
        }
        Ok(draft)
    }

    /// Adds a structure or union member, in declaration order.
    pub fn add_member(
        &mut self,
        parent: DraftId,
        name: &str,
        target: DraftId,
        traits: MemberTraits,
    ) -> Result<DraftId, BuildError> {
        let DraftChildren::Members(members) = &self.parent(parent)?.children else {
            return Err(BuildError::InvalidParent(self.fqn(parent)));
        };
        let count = members.len();
        if self.has_member(parent, name) {
            return Err(BuildError::DuplicateMember(
                format!("{}${name}", self.fqn(parent)).into(),
            ));
        }
        let member = self.new_member(parent, name, target, traits)?;
        if let DraftChildren::Members(members) = &mut self.drafts[parent.index()].children {
            members.push(member);
        }
        // A short member list is scanned; a long one is indexed, from the
        // member that makes it long, by each name's hash.
        if count + 1 == SCAN_MEMBERS {
            for index in 0..SCAN_MEMBERS {
                let hash = hash(self.member_name(parent, index));
                self.member_names.insert((parent, hash));
            }
        } else if count + 1 > SCAN_MEMBERS {
            self.member_names.insert((parent, hash(name)));
        }
        Ok(member)
    }

    /// Room for `additional` more members of a structure or union: a
    /// hint, which changes no result.
    pub fn reserve_members(&mut self, parent: DraftId, additional: usize) {
        let draft = self.drafts.get_mut(parent.index());
        if let Some(DraftChildren::Members(members)) = draft.map(|draft| &mut draft.children) {
            members.reserve_exact(additional);
        }
    }

    /// The name of a structure's `index`th member; `""` past its members.
    fn member_name(&self, parent: DraftId, index: usize) -> &str {
        let DraftChildren::Members(members) = &self.drafts[parent.index()].children else {
            return "";
        };
        match members
            .get(index)
            .map(|member| self.drafts[member.index()].role)
        {
            Some(DraftRole::Member { name, .. }) => self.strings.get(name),
            _ => "",
        }
    }

    /// Whether a structure already has a member of this name.
    fn has_member(&self, parent: DraftId, name: &str) -> bool {
        let DraftChildren::Members(members) = &self.drafts[parent.index()].children else {
            return false;
        };
        let count = members.len();
        if count >= SCAN_MEMBERS && !self.member_names.contains(&(parent, hash(name))) {
            return false;
        }
        members
            .iter()
            .any(|member| match self.drafts[member.index()].role {
                DraftRole::Member { name: id, .. } => self.strings.is(id, name),
                _ => false,
            })
    }

    /// Sets a list's member (`$member`).
    pub fn set_list_member(
        &mut self,
        list: DraftId,
        target: DraftId,
        traits: MemberTraits,
    ) -> Result<DraftId, BuildError> {
        match self.parent(list)?.children {
            DraftChildren::List(None) => {}
            DraftChildren::List(Some(_)) => {
                return Err(BuildError::DuplicateMember(
                    format!("{}$member", self.fqn(list)).into(),
                ));
            }
            _ => return Err(BuildError::InvalidParent(self.fqn(list))),
        }
        let member = self.new_member(list, "member", target, traits)?;
        self.drafts[list.index()].children = DraftChildren::List(Some(member));
        Ok(member)
    }

    /// Sets a map's key (`$key`) and value (`$value`).
    pub fn set_map_entry(
        &mut self,
        map: DraftId,
        key: (DraftId, MemberTraits),
        value: (DraftId, MemberTraits),
    ) -> Result<(DraftId, DraftId), BuildError> {
        match self.parent(map)?.children {
            DraftChildren::Map(None) => {}
            DraftChildren::Map(Some(_)) => {
                return Err(BuildError::DuplicateMember(
                    format!("{}$key", self.fqn(map)).into(),
                ));
            }
            _ => return Err(BuildError::InvalidParent(self.fqn(map))),
        }
        let key = self.new_member(map, "key", key.0, key.1)?;
        let value = self.new_member(map, "value", value.0, value.1)?;
        self.drafts[map.index()].children = DraftChildren::Map(Some((key, value)));
        Ok((key, value))
    }

    /// An operation reference's root: the target itself when the reference
    /// adds nothing, else an overlay sharing the target's members.
    pub fn root_overlay(
        &mut self,
        target: DraftId,
        traits: MemberTraits,
    ) -> Result<DraftId, BuildError> {
        self.check_target(target)?;
        if traits == MemberTraits::Inherit {
            return Ok(target);
        }
        let traits = self.resolved(traits)?;
        let draft = &self.drafts[target.index()];
        let overlay = Draft {
            role: DraftRole::Overlay { target },
            traits,
            children: draft.children.clone(),
            ..*draft
        };
        let id = DraftId(u32::try_from(self.drafts.len()).map_err(|_| BuildError::TooLarge)?);
        self.drafts.push(overlay);
        self.overlays.push(id);
        Ok(id)
    }

    /// Adds an operation, in model order.
    pub fn add_operation(
        &mut self,
        data: OperationData,
        shapes: OperationShapes,
    ) -> Result<OperationIndex, BuildError> {
        let (fqn, (sharp_at, dollar_at)) = self.with_qualified(&data.name, |_, fqn| {
            offsets(fqn).map(|at| (Box::<str>::from(fqn), at))
        })?;
        if dollar_at.is_some() {
            return Err(BuildError::InvalidShape(fqn));
        }
        if !self.operation_names.insert(data.name.clone()) {
            return Err(BuildError::DuplicateOperation(data.name));
        }
        let roots = shapes
            .input
            .iter()
            .chain(&shapes.output)
            .chain(&shapes.errors);
        for root in roots {
            if matches!(self.draft(*root)?.role, DraftRole::Member { .. }) {
                return Err(BuildError::InvalidTarget(self.fqn(*root)));
            }
        }
        let index = u32::try_from(self.operations.len()).map_err(|_| BuildError::TooLarge)?;
        self.operations.push((data, fqn, sharp_at, shapes));
        Ok(OperationIndex(index))
    }

    pub fn set_service(&mut self, service: ServiceData) {
        self.service = service;
    }

    /// Records a key no front-end rule interprets. [`Self::finish`] refuses
    /// one whose operation or shape the set does not have.
    pub fn record_uninterpreted(&mut self, site: Site, key: &str) {
        self.uninterpreted.push(Uninterpreted {
            site,
            key: key.into(),
        });
    }

    /// Checks the drafts and lays the set out.
    pub fn finish(self) -> Result<SchemaSet, BuildError> {
        layout::finish(self)
    }
}

#[cfg(test)]
mod tests;
