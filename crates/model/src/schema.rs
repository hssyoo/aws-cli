//! Smithy-shaped schemas: one owned arena per service ([`SchemaSet`]) and
//! `Copy` views over its indices ([`Schema`]), as smithy-rs's runtime
//! schemas read but without `unsafe` self-reference.
//!
//! A front-end (C2J now, Smithy later) fills a [`SchemaSetBuilder`]. The
//! set holds the named shapes first in document order, then the members,
//! the operations' root overlays and the [`PRELUDE`]. Every member and
//! overlay's traits are effective: the target's, overlaid by the
//! reference's, and a reference that changes nothing shares the target's.
use self::strings::StrPool;
use serde_json::Value as Json;

mod builder;
mod cold;
mod id;
mod known;
mod operation;
pub mod projection;
mod registry;
mod strings;
mod types;
mod uninterpreted;

pub use builder::{BuildError, DraftId, MemberTraits, OperationShapes, SchemaSetBuilder};
pub use cold::{C2J_NAMESPACE, ColdTraits};
pub use id::{ShapeId, ShapeIdError};
pub use known::{
    Bound, ErrorInfo, FLAGS, Flag, HttpBinding, KNOWN, KnownTraits, Retryable, Storage,
    TimestampFormat, TraitFlags, TraitId, XmlNamespace, traits,
};
pub use operation::{
    ClientContextParam, EndpointDiscovery, HttpChecksum, OperationData, OperationIndex,
    OperationSchema, ServiceData, ServiceSchema,
};
pub use registry::TypeRegistry;
pub use strings::StrId;
pub use types::{PRELUDE, PRELUDE_NAMESPACE, ShapeType};
pub use uninterpreted::{Site, Uninterpreted};

/// A cold map: each trait id text with an index into the set's nodes.
type TraitMap = Box<[(strings::StrId, u32)]>;

/// A schema's index in its set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SchemaId(u32);

impl SchemaId {
    const fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a schema is, beyond its type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Named,
    Member {
        name: strings::StrId,
        index: u32,
        target: SchemaId,
    },
    /// An operation's input, output or error reference with its own traits.
    Overlay,
    Prelude,
}

/// A schema's children. A member and an overlay carry their target's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Children {
    None,
    /// A range of the member table and, for 16 or more members, of the
    /// name-sorted lookup table.
    Members {
        start: u32,
        len: u32,
        lookup: Option<u32>,
    },
    List(SchemaId),
    Map(SchemaId, SchemaId),
}

#[derive(Clone, Debug, PartialEq)]
struct SchemaData {
    id: strings::StrId,
    sharp_at: u16,
    dollar_at: Option<u16>,
    shape_type: ShapeType,
    type_name: strings::StrId,
    role: Role,
    children: Children,
    known: u32,
    cold: u32,
    own_traits: bool,
}

/// One service's schemas, held in its `Model`.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaSet {
    strings: StrPool,
    /// Named shapes `[0, named)` in document order, then members, overlays
    /// and the prelude.
    schemas: Box<[SchemaData]>,
    named: u32,
    members: Box<[SchemaId]>,
    member_lookup: Box<[SchemaId]>,
    known: Box<[KnownTraits]>,
    /// Each cold map: trait id text and an index into `nodes`.
    cold: Box<[TraitMap]>,
    nodes: Box<[Json]>,
    /// The named range sorted by short name; never the prelude.
    by_name: Box<[SchemaId]>,
    operations: Box<[operation::OperationEntry]>,
    service: ServiceData,
    uninterpreted: Box<[Uninterpreted]>,
}

/// A schema: a `Copy` view of one entry of a [`SchemaSet`].
#[derive(Clone, Copy)]
pub struct Schema<'s> {
    set: &'s SchemaSet,
    id: SchemaId,
}

impl std::fmt::Debug for Schema<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Schema").field(&self.id().as_str()).finish()
    }
}

impl PartialEq for Schema<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.set, other.set) && self.id == other.id
    }
}

impl Eq for Schema<'_> {}

impl SchemaSet {
    pub fn schema(&self, id: SchemaId) -> Schema<'_> {
        Schema { set: self, id }
    }

    /// The number of schemas, prelude included.
    pub fn len(&self) -> usize {
        self.schemas.len()
    }

    /// Never true: the prelude is always present.
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty()
    }

    /// The named shapes in document order: the model's own, no member,
    /// overlay or prelude shape.
    pub fn named(&self) -> impl ExactSizeIterator<Item = Schema<'_>> + '_ {
        (0..self.named).map(|index| self.schema(SchemaId(index)))
    }

    /// `ServiceModel.shape_for(name)` without member traits: a named shape
    /// by its short name.
    pub fn shape_named(&self, name: &str) -> Option<Schema<'_>> {
        let found = self
            .by_name
            .binary_search_by(|id| self.schema(*id).id().name().cmp(name))
            .ok()?;
        Some(self.schema(self.by_name[found]))
    }

    /// A prelude shape by its name in [`PRELUDE`].
    pub fn prelude(&self, name: &str) -> Option<Schema<'_>> {
        let index = PRELUDE.iter().position(|(entry, _)| *entry == name)?;
        let start = self.schemas.len() - PRELUDE.len();
        Some(self.schema(SchemaId((start + index) as u32)))
    }

    pub const fn service(&self) -> ServiceSchema<'_> {
        ServiceSchema { set: self }
    }

    pub const fn registry(&self) -> TypeRegistry<'_> {
        TypeRegistry { set: self }
    }

    pub fn operation(&self, index: OperationIndex) -> OperationSchema<'_> {
        OperationSchema { set: self, index }
    }

    fn str(&self, id: strings::StrId) -> &str {
        self.strings.get(id)
    }
}

/// A structure's members, in declaration order.
#[derive(Clone, Debug)]
pub struct Members<'s> {
    set: &'s SchemaSet,
    ids: std::slice::Iter<'s, SchemaId>,
}

impl<'s> Iterator for Members<'s> {
    type Item = Schema<'s>;

    fn next(&mut self) -> Option<Schema<'s>> {
        self.ids.next().map(|id| self.set.schema(*id))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.ids.size_hint()
    }
}

impl ExactSizeIterator for Members<'_> {}

/// An `xmlNamespace`, as texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XmlNamespaceView<'s> {
    pub uri: Option<&'s str>,
    pub prefix: Option<&'s str>,
}

/// An `error` object, as texts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorView<'s> {
    pub code: Option<&'s str>,
    pub http_status: Option<u16>,
    pub sender_fault: bool,
}

/// One `&'s str` accessor per text slot of [`KnownTraits`].
macro_rules! text_accessors {
    ($($(#[$doc:meta])* $field:ident),* $(,)?) => {
        impl<'s> Schema<'s> {
            $(
                $(#[$doc])*
                pub fn $field(self) -> Option<&'s str> {
                    self.known().$field.map(|id| self.set.str(id))
                }
            )*
        }
    };
}

text_accessors! {
    /// Raw `location`.
    location,
    /// Raw `locationName`.
    location_name,
    json_name,
    xml_name,
    ec2_query_name,
    payload,
    pattern,
    context_param,
    result_wrapper,
}

impl<'s> Schema<'s> {
    fn data(self) -> &'s SchemaData {
        &self.set.schemas[self.id.index()]
    }

    pub const fn index(self) -> SchemaId {
        self.id
    }

    pub fn id(self) -> ShapeId<'s> {
        let data = self.data();
        ShapeId::from_parts(self.set.str(data.id), data.sharp_at, data.dollar_at)
    }

    /// A member reports its target's type.
    pub fn shape_type(self) -> ShapeType {
        self.data().shape_type
    }

    /// botocore's `type` text.
    pub fn type_name(self) -> &'s str {
        self.set.str(self.data().type_name)
    }

    pub fn is_member(self) -> bool {
        matches!(self.data().role, Role::Member { .. })
    }

    pub fn is_prelude(self) -> bool {
        self.data().role == Role::Prelude
    }

    pub fn member_name(self) -> Option<&'s str> {
        match self.data().role {
            Role::Member { name, .. } => Some(self.set.str(name)),
            _ => None,
        }
    }

    /// The member's position among its container's members.
    pub fn member_index(self) -> Option<usize> {
        match self.data().role {
            Role::Member { index, .. } => Some(index as usize),
            _ => None,
        }
    }

    /// A member's target; any other schema itself.
    pub fn target(self) -> Self {
        match self.data().role {
            Role::Member { target, .. } => self.set.schema(target),
            _ => self,
        }
    }

    /// A structure or union's members in declaration order; empty for any
    /// other type.
    pub fn members(self) -> Members<'s> {
        let ids = match self.data().children {
            Children::Members { start, len, .. } => {
                &self.set.members[start as usize..(start + len) as usize]
            }
            _ => &[],
        };
        Members {
            set: self.set,
            ids: ids.iter(),
        }
    }

    /// A member by name: a scan for under 16 members, else a binary search.
    pub fn member(self, name: &str) -> Option<Self> {
        let Children::Members { len, lookup, .. } = self.data().children else {
            return None;
        };
        let Some(start) = lookup else {
            return self
                .members()
                .find(|member| member.member_name() == Some(name));
        };
        let sorted = &self.set.member_lookup[start as usize..(start + len) as usize];
        let by_name = |id: &SchemaId| self.set.schema(*id).member_name().unwrap_or("").cmp(name);
        let found = sorted.binary_search_by(by_name).ok()?;
        Some(self.set.schema(sorted[found]))
    }

    pub fn list_member(self) -> Option<Self> {
        match self.data().children {
            Children::List(member) => Some(self.set.schema(member)),
            _ => None,
        }
    }

    pub fn map_key(self) -> Option<Self> {
        match self.data().children {
            Children::Map(key, _) => Some(self.set.schema(key)),
            _ => None,
        }
    }

    pub fn map_value(self) -> Option<Self> {
        match self.data().children {
            Children::Map(_, value) => Some(self.set.schema(value)),
            _ => None,
        }
    }

    /// The effective slots: the target's, overlaid by the reference's.
    pub fn known(self) -> &'s KnownTraits {
        &self.set.known[self.data().known as usize]
    }

    pub fn flag(self, flag: Flag) -> bool {
        self.known().flags.is(flag)
    }

    pub fn http_binding(self) -> HttpBinding<'s> {
        HttpBinding::of(self.location(), self.location_name())
    }

    /// Raw `timestampFormat`; [`Self::timestamp_format`] is the view.
    pub fn timestamp_format_text(self) -> Option<&'s str> {
        self.known().timestamp_format.map(|id| self.set.str(id))
    }

    pub fn timestamp_format(self) -> Option<TimestampFormat<'s>> {
        self.timestamp_format_text().map(TimestampFormat::of)
    }

    pub fn xml_namespace(self) -> Option<XmlNamespaceView<'s>> {
        let namespace = self.known().xml_namespace?;
        Some(XmlNamespaceView {
            uri: namespace.uri.map(|id| self.set.str(id)),
            prefix: namespace.prefix.map(|id| self.set.str(id)),
        })
    }

    /// The required names in botocore's list order.
    pub fn required(self) -> impl ExactSizeIterator<Item = &'s str> + 's {
        self.texts(self.known().required.as_deref())
    }

    /// The enum values in model order.
    pub fn enum_values(self) -> impl ExactSizeIterator<Item = &'s str> + 's {
        self.texts(self.known().enum_values.as_deref())
    }

    fn texts(self, list: Option<&'s [StrId]>) -> impl ExactSizeIterator<Item = &'s str> + 's {
        let set = self.set;
        list.unwrap_or_default().iter().map(move |id| set.str(*id))
    }

    pub fn min(self) -> Option<Bound> {
        self.known().min
    }

    pub fn max(self) -> Option<Bound> {
        self.known().max
    }

    pub fn error(self) -> Option<ErrorView<'s>> {
        let error = self.known().error?;
        Some(ErrorView {
            code: error.code.map(|id| self.set.str(id)),
            http_status: error.http_status,
            sender_fault: error.sender_fault,
        })
    }

    pub fn retryable(self) -> Option<Retryable> {
        self.known().retryable
    }

    /// Whether the schema carries a trait: its slot when [`KNOWN`] stores it
    /// in one, else the cold map. An operation or service trait is false.
    pub fn has_trait(self, id: TraitId) -> bool {
        match id.storage() {
            Storage::Slot => self.known().has_slot(id, self.shape_type()) == Some(true),
            Storage::Cold | Storage::Ignored => self.cold_trait(id.as_str()).is_some(),
        }
    }

    /// A trait of the cold map, by id text. Slot traits are never here.
    pub fn cold_trait(self, id: &str) -> Option<&'s Json> {
        let map = &self.set.cold[self.data().cold as usize];
        let found = map.iter().find(|(key, _)| self.set.str(*key) == id)?;
        Some(&self.set.nodes[found.1 as usize])
    }

    /// The cold map's ids and values, in insertion order.
    pub fn cold_traits(self) -> impl ExactSizeIterator<Item = (&'s str, &'s Json)> + 's {
        let set = self.set;
        let map = &set.cold[self.data().cold as usize];
        map.iter()
            .map(move |(key, node)| (set.str(*key), &set.nodes[*node as usize]))
    }

    /// Whether a member or root overlay's traits differ from its target's.
    pub fn has_own_traits(self) -> bool {
        self.data().own_traits
    }
}

#[cfg(test)]
mod tests;
