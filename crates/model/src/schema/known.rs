//! Trait storage: Smithy trait ids, the typed slots code reads, and the
//! views over the raw texts.
//!
//! Every C2J key or Smithy trait lands in exactly one place: a slot, if any
//! code reads it, else the cold map. [`KNOWN`] says which for each known id;
//! an id it does not list is unknown, kept in the cold map and recorded as
//! uninterpreted (see [`super::ColdTraits::insert_unknown`]).
use super::strings::StrId;
use super::types::ShapeType;
use crate::pyrepr::float_repr;
use std::sync::Arc;

/// A known trait's Smithy shape id: an index into [`KNOWN`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraitId(u16);

/// Where a known trait is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Storage {
    /// A typed slot: of [`KnownTraits`], or of an operation or the service.
    Slot,
    /// The cold map: kept, read by nothing.
    Cold,
    /// Accepted and stored nowhere: read by nothing, or translated into
    /// other slots; a front-end may drop it or keep it cold.
    Ignored,
}

/// One list generates [`KNOWN`] and the [`traits`] constants, so adding a
/// trait cannot shift a hand-written index. Entries are only appended.
macro_rules! known_traits {
    ($($name:ident = $id:literal => $storage:ident,)*) => {
        /// Every known trait id with its storage, in [`TraitId`] order.
        pub const KNOWN: &[(&str, Storage)] = &[$(($id, Storage::$storage),)*];
        /// One constant per [`KNOWN`] entry.
        pub mod traits {
            use super::TraitId;
            known_traits!(@ids 0; $($name)*);
        }
    };
    (@ids $index:expr; $name:ident $($rest:ident)*) => {
        pub const $name: TraitId = TraitId($index);
        known_traits!(@ids $index + 1; $($rest)*);
    };
    (@ids $index:expr;) => {};
}

known_traits! {
    SENSITIVE = "smithy.api#sensitive" => Slot,
    XML_ATTRIBUTE = "smithy.api#xmlAttribute" => Slot,
    XML_FLATTENED = "smithy.api#xmlFlattened" => Slot,
    STREAMING = "smithy.api#streaming" => Slot,
    EVENT_STREAM = "c2j#eventstream" => Slot,
    EVENT_HEADER = "smithy.api#eventHeader" => Slot,
    EVENT_PAYLOAD = "smithy.api#eventPayload" => Slot,
    HOST_LABEL = "smithy.api#hostLabel" => Slot,
    IDEMPOTENCY_TOKEN = "smithy.api#idempotencyToken" => Slot,
    ERROR = "smithy.api#error" => Slot,
    REQUIRES_LENGTH = "smithy.api#requiresLength" => Slot,
    DEPRECATED = "smithy.api#deprecated" => Slot,
    ENDPOINT_DISCOVERY_ID = "aws.api#clientEndpointDiscoveryId" => Slot,
    JSON_VALUE = "c2j#jsonvalue" => Slot,
    LOCATION = "c2j#location" => Slot,
    LOCATION_NAME = "c2j#locationName" => Slot,
    JSON_NAME = "smithy.api#jsonName" => Slot,
    XML_NAME = "smithy.api#xmlName" => Slot,
    EC2_QUERY_NAME = "aws.protocols#ec2QueryName" => Slot,
    HTTP_PAYLOAD = "smithy.api#httpPayload" => Slot,
    TIMESTAMP_FORMAT = "smithy.api#timestampFormat" => Slot,
    XML_NAMESPACE = "smithy.api#xmlNamespace" => Slot,
    REQUIRED = "smithy.api#required" => Slot,
    ENUM = "smithy.api#enum" => Slot,
    LENGTH = "smithy.api#length" => Slot,
    RANGE = "smithy.api#range" => Slot,
    PATTERN = "smithy.api#pattern" => Slot,
    HTTP_ERROR = "smithy.api#httpError" => Slot,
    AWS_QUERY_ERROR = "aws.protocols#awsQueryError" => Slot,
    RETRYABLE = "smithy.api#retryable" => Slot,
    CONTEXT_PARAM = "smithy.rules#contextParam" => Slot,
    RESULT_WRAPPER = "c2j#resultWrapper" => Slot,
    HTTP = "smithy.api#http" => Slot,
    ENDPOINT = "smithy.api#endpoint" => Slot,
    AUTH = "smithy.api#auth" => Slot,
    AUTH_TYPE = "c2j#authtype" => Slot,
    UNSIGNED_PAYLOAD = "aws.auth#unsignedPayload" => Slot,
    HTTP_CHECKSUM_REQUIRED = "smithy.api#httpChecksumRequired" => Slot,
    HTTP_CHECKSUM = "aws.protocols#httpChecksum" => Slot,
    REQUEST_COMPRESSION = "smithy.api#requestCompression" => Slot,
    CLIENT_DISCOVERED_ENDPOINT = "aws.api#clientDiscoveredEndpoint" => Slot,
    ENDPOINT_OPERATION = "c2j#endpointoperation" => Slot,
    STATIC_CONTEXT_PARAMS = "smithy.rules#staticContextParams" => Slot,
    OPERATION_CONTEXT_PARAMS = "smithy.rules#operationContextParams" => Slot,
    AWS_SERVICE = "aws.api#service" => Slot,
    SIGV4 = "aws.auth#sigv4" => Slot,
    AWS_QUERY_COMPATIBLE = "aws.protocols#awsQueryCompatible" => Slot,
    CLIENT_CONTEXT_PARAMS = "smithy.rules#clientContextParams" => Slot,
    AWS_JSON_1_0 = "aws.protocols#awsJson1_0" => Slot,
    AWS_JSON_1_1 = "aws.protocols#awsJson1_1" => Slot,
    REST_JSON_1 = "aws.protocols#restJson1" => Slot,
    REST_XML = "aws.protocols#restXml" => Slot,
    AWS_QUERY = "aws.protocols#awsQuery" => Slot,
    EC2_QUERY = "aws.protocols#ec2Query" => Slot,
    RPC_V2_CBOR = "smithy.protocols#rpcv2Cbor" => Slot,
    EVENT = "c2j#event" => Cold,
    DOCUMENT = "c2j#document" => Cold,
    UNION = "c2j#union" => Cold,
    MEDIA_TYPE = "smithy.api#mediaType" => Cold,
    DOCUMENTATION = "smithy.api#documentation" => Ignored,
    EXAMPLES = "smithy.api#examples" => Ignored,
    TITLE = "smithy.api#title" => Ignored,
    IDEMPOTENT = "smithy.api#idempotent" => Ignored,
    READONLY = "smithy.api#readonly" => Ignored,
    SPARSE = "smithy.api#sparse" => Ignored,
    TAGS = "smithy.api#tags" => Ignored,
    // Structure markers and defaults botocore never reads: it applies no
    // default (a non-goal here too).
    INPUT = "smithy.api#input" => Ignored,
    OUTPUT = "smithy.api#output" => Ignored,
    DEFAULT = "smithy.api#default" => Ignored,
    // The HTTP bindings: a Smithy front-end translates each into the raw
    // `c2j#location` and `c2j#locationName` slots and stores the id itself
    // nowhere, so no model refuses on them and `has_trait` answers false
    // for them (translated, not queryable: `Schema::http_binding` is the
    // read).
    HTTP_HEADER = "smithy.api#httpHeader" => Ignored,
    HTTP_LABEL = "smithy.api#httpLabel" => Ignored,
    HTTP_QUERY = "smithy.api#httpQuery" => Ignored,
    HTTP_QUERY_PARAMS = "smithy.api#httpQueryParams" => Ignored,
    HTTP_PREFIX_HEADERS = "smithy.api#httpPrefixHeaders" => Ignored,
    HTTP_RESPONSE_CODE = "smithy.api#httpResponseCode" => Ignored,
}

impl TraitId {
    /// The known id with this text; `None` for an unknown trait.
    pub fn parse(text: &str) -> Option<Self> {
        let index = KNOWN.iter().position(|(id, _)| *id == text)?;
        u16::try_from(index).ok().map(Self)
    }

    pub fn as_str(self) -> &'static str {
        KNOWN[usize::from(self.0)].0
    }

    pub fn storage(self) -> Storage {
        KNOWN[usize::from(self.0)].1
    }
}

/// A boolean trait, as a bit of [`TraitFlags`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Flag {
    Sensitive = 1 << 0,
    XmlAttribute = 1 << 1,
    XmlFlattened = 1 << 2,
    Streaming = 1 << 3,
    EventStream = 1 << 4,
    EventHeader = 1 << 5,
    EventPayload = 1 << 6,
    HostLabel = 1 << 7,
    IdempotencyToken = 1 << 8,
    Error = 1 << 9,
    RequiresLength = 1 << 10,
    Deprecated = 1 << 11,
    EndpointDiscoveryId = 1 << 12,
    JsonValue = 1 << 13,
}

/// Each flag with its trait id.
pub const FLAGS: [(Flag, TraitId); 14] = [
    (Flag::Sensitive, traits::SENSITIVE),
    (Flag::XmlAttribute, traits::XML_ATTRIBUTE),
    (Flag::XmlFlattened, traits::XML_FLATTENED),
    (Flag::Streaming, traits::STREAMING),
    (Flag::EventStream, traits::EVENT_STREAM),
    (Flag::EventHeader, traits::EVENT_HEADER),
    (Flag::EventPayload, traits::EVENT_PAYLOAD),
    (Flag::HostLabel, traits::HOST_LABEL),
    (Flag::IdempotencyToken, traits::IDEMPOTENCY_TOKEN),
    (Flag::Error, traits::ERROR),
    (Flag::RequiresLength, traits::REQUIRES_LENGTH),
    (Flag::Deprecated, traits::DEPRECATED),
    (Flag::EndpointDiscoveryId, traits::ENDPOINT_DISCOVERY_ID),
    (Flag::JsonValue, traits::JSON_VALUE),
];

/// The boolean traits a schema carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TraitFlags(u32);

impl TraitFlags {
    pub const fn is(self, flag: Flag) -> bool {
        self.0 & flag as u32 != 0
    }

    pub fn set(&mut self, flag: Flag, on: bool) {
        if on {
            self.0 |= flag as u32;
        } else {
            self.0 &= !(flag as u32);
        }
    }
}

/// A `min` or `max` as the number the model wrote: `str()` prints an
/// integer bound without a fraction and a float one with Python's `repr`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bound {
    Integer(i64),
    Float(f64),
}

impl std::fmt::Display for Bound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Integer(value) => value.fmt(f),
            Self::Float(value) => f.write_str(&float_repr(*value)),
        }
    }
}

/// An `xmlNamespace`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct XmlNamespace {
    pub uri: Option<StrId>,
    pub prefix: Option<StrId>,
}

/// An error shape's `error` object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ErrorInfo {
    pub code: Option<StrId>,
    pub http_status: Option<u16>,
    pub sender_fault: bool,
}

/// An error shape's `retryable` object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Retryable {
    pub throttling: bool,
}

/// The typed slots, filled by a front-end with botocore's meaning. Texts
/// are raw: no defaults are applied, each consumer applies botocore's own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KnownTraits {
    pub flags: TraitFlags,
    /// Raw C2J `location`.
    pub location: Option<StrId>,
    /// Raw C2J `locationName` (botocore's `serialization['name']`).
    pub location_name: Option<StrId>,
    pub json_name: Option<StrId>,
    pub xml_name: Option<StrId>,
    pub ec2_query_name: Option<StrId>,
    /// A structure's payload member name; it may name no member.
    pub payload: Option<StrId>,
    /// Raw text; [`TimestampFormat`] is the view.
    pub timestamp_format: Option<StrId>,
    pub xml_namespace: Option<XmlNamespace>,
    /// A structure's required names in botocore's list order; they may name
    /// non-members.
    pub required: Option<Arc<[StrId]>>,
    pub enum_values: Option<Arc<[StrId]>>,
    pub min: Option<Bound>,
    pub max: Option<Bound>,
    pub pattern: Option<StrId>,
    pub error: Option<ErrorInfo>,
    pub retryable: Option<Retryable>,
    pub context_param: Option<StrId>,
    /// An operation output root's awsQuery `resultWrapper`.
    pub result_wrapper: Option<StrId>,
}

impl KnownTraits {
    /// Smithy's member-over-target merge, by trait id: `self` is the
    /// target's set, `over` the member's; the member wins and flags only add.
    /// The C2J front-end merges keys before translating instead (it can
    /// clear a flag), so it passes resolved sets.
    #[must_use]
    pub fn overlaid(&self, over: &Self) -> Self {
        fn pick<T: Clone>(over: &Option<T>, under: &Option<T>) -> Option<T> {
            over.as_ref().or(under.as_ref()).cloned()
        }
        Self {
            flags: TraitFlags(self.flags.0 | over.flags.0),
            location: pick(&over.location, &self.location),
            location_name: pick(&over.location_name, &self.location_name),
            json_name: pick(&over.json_name, &self.json_name),
            xml_name: pick(&over.xml_name, &self.xml_name),
            ec2_query_name: pick(&over.ec2_query_name, &self.ec2_query_name),
            payload: pick(&over.payload, &self.payload),
            timestamp_format: pick(&over.timestamp_format, &self.timestamp_format),
            xml_namespace: pick(&over.xml_namespace, &self.xml_namespace),
            required: pick(&over.required, &self.required),
            enum_values: pick(&over.enum_values, &self.enum_values),
            min: pick(&over.min, &self.min),
            max: pick(&over.max, &self.max),
            pattern: pick(&over.pattern, &self.pattern),
            error: pick(&over.error, &self.error),
            retryable: pick(&over.retryable, &self.retryable),
            context_param: pick(&over.context_param, &self.context_param),
            result_wrapper: pick(&over.result_wrapper, &self.result_wrapper),
        }
    }

    /// Every string the slots name, for the builder's range check.
    pub(super) fn strings(&self) -> impl Iterator<Item = StrId> + '_ {
        let single = [
            self.location,
            self.location_name,
            self.json_name,
            self.xml_name,
            self.ec2_query_name,
            self.payload,
            self.timestamp_format,
            self.pattern,
            self.context_param,
            self.result_wrapper,
            self.xml_namespace.and_then(|ns| ns.uri),
            self.xml_namespace.and_then(|ns| ns.prefix),
            self.error.and_then(|error| error.code),
        ];
        let lists = self.required.iter().chain(&self.enum_values);
        single
            .into_iter()
            .flatten()
            .chain(lists.flat_map(|list| list.iter().copied()))
    }

    /// Whether the slot of a shape-level trait is filled; `None` for an id
    /// with no shape slot (operation, service, cold or ignored ids).
    pub(super) fn has_slot(&self, id: TraitId, shape_type: ShapeType) -> Option<bool> {
        if let Some((flag, _)) = FLAGS.iter().find(|(_, flag_id)| *flag_id == id) {
            return Some(self.flags.is(*flag));
        }
        let bounded = self.min.is_some() || self.max.is_some();
        let present = match id {
            traits::LOCATION => self.location.is_some(),
            traits::LOCATION_NAME => self.location_name.is_some(),
            traits::JSON_NAME => self.json_name.is_some(),
            traits::XML_NAME => self.xml_name.is_some(),
            traits::EC2_QUERY_NAME => self.ec2_query_name.is_some(),
            traits::HTTP_PAYLOAD => self.payload.is_some(),
            traits::TIMESTAMP_FORMAT => self.timestamp_format.is_some(),
            traits::XML_NAMESPACE => self.xml_namespace.is_some(),
            traits::REQUIRED => self.required.is_some(),
            traits::ENUM => self.enum_values.is_some(),
            traits::LENGTH => bounded && !shape_type.is_number(),
            traits::RANGE => bounded && shape_type.is_number(),
            traits::PATTERN => self.pattern.is_some(),
            traits::HTTP_ERROR => self.error.is_some_and(|e| e.http_status.is_some()),
            traits::AWS_QUERY_ERROR => self.error.is_some_and(|e| e.code.is_some()),
            traits::RETRYABLE => self.retryable.is_some(),
            traits::CONTEXT_PARAM => self.context_param.is_some(),
            traits::RESULT_WRAPPER => self.result_wrapper.is_some(),
            _ => return None,
        };
        Some(present)
    }
}

/// A member's HTTP binding, derived from raw `location` and
/// `locationName`, with no defaults: each consumer applies botocore's (the
/// headers prefix is the member name when serializing, `""` when parsing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpBinding<'s> {
    Body,
    Header(Option<&'s str>),
    PrefixHeaders(Option<&'s str>),
    Query(Option<&'s str>),
    Label(Option<&'s str>),
    ResponseCode(Option<&'s str>),
    /// A `location` botocore has no serializer branch for.
    Other(&'s str),
}

impl<'s> HttpBinding<'s> {
    pub(super) fn of(location: Option<&'s str>, name: Option<&'s str>) -> Self {
        match location {
            None => Self::Body,
            Some("header") => Self::Header(name),
            Some("headers") => Self::PrefixHeaders(name),
            Some("querystring") => Self::Query(name),
            Some("uri") => Self::Label(name),
            Some("statusCode") => Self::ResponseCode(name),
            Some(other) => Self::Other(other),
        }
    }
}

/// A view over a raw `timestampFormat`. botocore lower-cases the text at
/// use, so the known names match regardless of ASCII case (no non-ASCII
/// text lower-cases to one of them); anything else is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimestampFormat<'s> {
    Iso8601,
    UnixTimestamp,
    Rfc822,
    Other(&'s str),
}

impl<'s> TimestampFormat<'s> {
    pub fn of(text: &'s str) -> Self {
        let named = [
            ("iso8601", Self::Iso8601),
            ("unixtimestamp", Self::UnixTimestamp),
            ("rfc822", Self::Rfc822),
        ];
        named
            .into_iter()
            .find(|(name, _)| text.eq_ignore_ascii_case(name))
            .map_or(Self::Other(text), |(_, format)| format)
    }
}

#[cfg(test)]
mod tests;
