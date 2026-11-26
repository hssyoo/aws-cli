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
use super::strings::{Interner, StrId};
use super::types::{PRELUDE, PRELUDE_NAMESPACE, ShapeType};
use super::uninterpreted::{Site, Uninterpreted};
use std::collections::{HashMap, HashSet};

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

impl From<BuildError> for crate::shapes::ModelError {
    fn from(error: BuildError) -> Self {
        match error {
            BuildError::NoShapeFound(name) => Self::NoShapeFound(name),
            BuildError::TooLarge => Self::TooLarge,
            other => Self::InvalidShape(other.to_string().into()),
        }
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

#[derive(Clone, Debug, PartialEq)]
enum DraftTraits {
    Own(Box<KnownTraits>, ColdEntries),
    Inherit,
}

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
    by_fqn: HashMap<Box<str>, DraftId>,
    /// Named shapes in declaration order.
    order: Vec<DraftId>,
    overlays: Vec<DraftId>,
    operations: Vec<(OperationData, Box<str>, u16, OperationShapes)>,
    operation_names: HashSet<Box<str>>,
    service: ServiceData,
    uninterpreted: Vec<Uninterpreted>,
}

fn own_traits() -> DraftTraits {
    DraftTraits::Own(Box::default(), Vec::new())
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
            by_fqn: HashMap::new(),
            order: Vec::new(),
            overlays: Vec::new(),
            operations: Vec::new(),
            operation_names: HashSet::new(),
            service: ServiceData::default(),
            uninterpreted: Vec::new(),
        };
        for (name, shape_type) in PRELUDE {
            let fqn = format!("{PRELUDE_NAMESPACE}#{name}");
            let type_name = builder.default_type_name(&fqn, shape_type, None)?;
            let draft = builder.push(&fqn, shape_type, Some(type_name), DraftRole::Prelude)?;
            builder.by_fqn.insert(fqn.into(), draft);
        }
        Ok(builder)
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
        let (sharp_at, dollar_at) = offsets(fqn)?;
        let draft = Draft {
            id: self.intern(fqn)?,
            sharp_at,
            dollar_at,
            shape_type,
            type_name,
            role,
            children: children_for(shape_type),
            traits: own_traits(),
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

    fn qualified(&self, name: &str) -> String {
        format!("{}#{name}", self.namespace)
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
        let fqn = self.qualified(name);
        self.declare_id(&fqn, shape_type, type_name)
    }

    /// Declares a named shape by its full id.
    pub fn declare_id(
        &mut self,
        fqn: &str,
        shape_type: ShapeType,
        type_name: Option<&str>,
    ) -> Result<DraftId, BuildError> {
        if offsets(fqn)?.1.is_some() {
            return Err(BuildError::InvalidShape(fqn.into()));
        }
        let type_name = self.default_type_name(fqn, shape_type, type_name)?;
        let id = match self.by_fqn.get(fqn) {
            Some(&id) if self.drafts[id.index()].role == DraftRole::Placeholder => id,
            Some(_) => return Err(BuildError::Duplicate(fqn.into())),
            None => {
                let id = self.push(fqn, shape_type, None, DraftRole::Placeholder)?;
                self.by_fqn.insert(fqn.into(), id);
                id
            }
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
        let fqn = self.qualified(name);
        self.reference_id(&fqn)
    }

    pub fn reference_id(&mut self, fqn: &str) -> Result<DraftId, BuildError> {
        if let Some(id) = self.by_fqn.get(fqn) {
            return Ok(*id);
        }
        if offsets(fqn)?.1.is_some() {
            return Err(BuildError::InvalidShape(fqn.into()));
        }
        let id = self.push(fqn, ShapeType::Unknown, None, DraftRole::Placeholder)?;
        self.by_fqn.insert(fqn.into(), id);
        Ok(id)
    }

    /// A prelude shape by its [`PRELUDE`] name.
    pub fn prelude(&self, name: &str) -> Option<DraftId> {
        self.by_fqn
            .get(&*format!("{PRELUDE_NAMESPACE}#{name}"))
            .copied()
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

    fn resolved(&mut self, traits: MemberTraits) -> DraftTraits {
        match traits {
            MemberTraits::Inherit => DraftTraits::Inherit,
            MemberTraits::Resolved(known, cold) => DraftTraits::Own(known, self.take_cold(cold)),
        }
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
        self.drafts[id.index()].traits = DraftTraits::Own(Box::new(known), entries);
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
        let fqn = format!("{}${name}", self.fqn(container));
        let role = DraftRole::Member {
            container,
            name: self.intern(name)?,
            target,
        };
        let id = self.push(&fqn, ShapeType::Unknown, None, role)?;
        self.drafts[id.index()].traits = self.resolved(traits);
        Ok(id)
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
        let taken = members.iter().any(|member| {
            let role = self.drafts[member.index()].role;
            matches!(role, DraftRole::Member { name: n, .. } if self.strings.get(n) == name)
        });
        if taken {
            return Err(BuildError::DuplicateMember(
                format!("{}${name}", self.fqn(parent)).into(),
            ));
        }
        let member = self.new_member(parent, name, target, traits)?;
        if let DraftChildren::Members(members) = &mut self.drafts[parent.index()].children {
            members.push(member);
        }
        Ok(member)
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
        let draft = self.drafts[target.index()].clone();
        let overlay = Draft {
            role: DraftRole::Overlay { target },
            traits: self.resolved(traits),
            ..draft
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
        let fqn = self.qualified(&data.name);
        let (sharp_at, dollar_at) = offsets(&fqn)?;
        if dollar_at.is_some() {
            return Err(BuildError::InvalidShape(fqn.into()));
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
        self.operations.push((data, fqn.into(), sharp_at, shapes));
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
