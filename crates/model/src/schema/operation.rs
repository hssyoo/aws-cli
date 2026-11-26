//! Operation and service schemas: botocore's `OperationModel` and
//! `ServiceModel` reads, over the set.
use super::id::ShapeId;
use super::known::TraitId;
use super::{Schema, SchemaId, SchemaSet};
use crate::value::Value;

/// An operation's index in its set, in model order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationIndex(pub(super) u32);

/// An operation's `httpChecksum` object.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpChecksum {
    pub request_algorithm_member: Option<Box<str>>,
    pub request_checksum_required: bool,
    pub request_validation_mode_member: Option<Box<str>>,
    pub response_algorithms: Box<[Box<str>]>,
}

/// An operation's `endpointdiscovery` object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EndpointDiscovery {
    pub required: bool,
}

/// What a front-end reads of an operation besides its shapes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OperationData {
    /// The key in `operations`, the name callers use.
    pub name: Box<str>,
    /// C2J `name`, sent on the wire.
    pub wire_name: Option<Box<str>>,
    /// `http.method` and `http.requestUri`, verbatim.
    pub http_method: Option<Box<str>>,
    pub http_request_uri: Option<Box<str>>,
    /// `endpoint.hostPrefix`.
    pub host_prefix: Option<Box<str>>,
    pub auth: Option<Box<[Box<str>]>>,
    /// Raw C2J `authtype` (`c2j#authtype`).
    pub auth_type: Option<Box<str>>,
    pub unsigned_payload: bool,
    pub http_checksum_required: bool,
    /// `endpointoperation` (`c2j#endpointoperation`).
    pub endpoint_operation: bool,
    pub http_checksum: Option<HttpChecksum>,
    pub endpoint_discovery: Option<EndpointDiscovery>,
    /// `requestcompression.encodings`.
    pub request_compression: Option<Box<[Box<str>]>>,
    /// Each name with its `value` (null when absent).
    pub static_context_params: Box<[(Box<str>, Value)]>,
    /// Each name with its `path`.
    pub operation_context_params: Box<[(Box<str>, Box<str>)]>,
}

/// An operation as the set holds it: its data and its resolved shapes.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OperationEntry {
    pub(super) data: OperationData,
    pub(super) id: Box<str>,
    pub(super) sharp_at: u16,
    pub(super) input: Option<SchemaId>,
    pub(super) output: Option<SchemaId>,
    pub(super) errors: Box<[SchemaId]>,
}

/// One of the service's `clientContextParams`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientContextParam {
    pub name: Box<str>,
    pub type_name: Box<str>,
}

/// What a front-end reads of the service.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceData {
    pub version: Option<Box<str>>,
    pub sdk_id: Option<Box<str>>,
    pub endpoint_prefix: Option<Box<str>>,
    /// C2J `signingName`, unset when absent.
    pub sigv4_name: Option<Box<str>>,
    pub auth: Option<Box<[Box<str>]>>,
    pub xml_namespace: Option<Box<str>>,
    pub query_compatible: bool,
    /// The protocol traits as a set; priority lives in the overlay.
    pub protocol_traits: Box<[TraitId]>,
    pub client_context_params: Box<[ClientContextParam]>,
}

/// An operation, as smithy-rs's `OperationSchema`.
#[derive(Clone, Copy)]
pub struct OperationSchema<'s> {
    pub(super) set: &'s SchemaSet,
    pub(super) index: OperationIndex,
}

impl std::fmt::Debug for OperationSchema<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OperationSchema")
            .field(&self.name())
            .finish()
    }
}

fn texts(list: Option<&[Box<str>]>) -> Option<impl ExactSizeIterator<Item = &str>> {
    list.map(|items| items.iter().map(|item| &**item))
}

impl<'s> OperationSchema<'s> {
    fn entry(self) -> &'s OperationEntry {
        &self.set.operations[self.index.0 as usize]
    }

    fn data(self) -> &'s OperationData {
        &self.entry().data
    }

    pub const fn index(self) -> OperationIndex {
        self.index
    }

    pub fn id(self) -> ShapeId<'s> {
        let entry = self.entry();
        ShapeId::from_parts(&entry.id, entry.sharp_at, None)
    }

    pub fn name(self) -> &'s str {
        &self.data().name
    }

    pub fn wire_name(self) -> Option<&'s str> {
        self.data().wire_name.as_deref()
    }

    /// The input root: an overlay when the reference carries traits.
    pub fn input(self) -> Option<Schema<'s>> {
        self.entry().input.map(|id| self.set.schema(id))
    }

    pub fn output(self) -> Option<Schema<'s>> {
        self.entry().output.map(|id| self.set.schema(id))
    }

    /// The errors as written (botocore's order).
    pub fn errors(self) -> impl ExactSizeIterator<Item = Schema<'s>> + 's {
        let set = self.set;
        self.entry().errors.iter().map(move |id| set.schema(*id))
    }

    pub fn http_method(self) -> Option<&'s str> {
        self.data().http_method.as_deref()
    }

    pub fn http_request_uri(self) -> Option<&'s str> {
        self.data().http_request_uri.as_deref()
    }

    pub fn host_prefix(self) -> Option<&'s str> {
        self.data().host_prefix.as_deref()
    }

    pub fn auth(self) -> Option<impl ExactSizeIterator<Item = &'s str> + 's> {
        texts(self.data().auth.as_deref())
    }

    pub fn auth_type(self) -> Option<&'s str> {
        self.data().auth_type.as_deref()
    }

    pub fn unsigned_payload(self) -> bool {
        self.data().unsigned_payload
    }

    pub fn http_checksum_required(self) -> bool {
        self.data().http_checksum_required
    }

    pub fn endpoint_operation(self) -> bool {
        self.data().endpoint_operation
    }

    pub fn http_checksum(self) -> Option<&'s HttpChecksum> {
        self.data().http_checksum.as_ref()
    }

    pub fn endpoint_discovery(self) -> Option<EndpointDiscovery> {
        self.data().endpoint_discovery
    }

    pub fn request_compression(self) -> Option<impl ExactSizeIterator<Item = &'s str> + 's> {
        texts(self.data().request_compression.as_deref())
    }

    pub fn static_context_params(self) -> impl ExactSizeIterator<Item = (&'s str, &'s Value)> {
        let params = self.data().static_context_params.iter();
        params.map(|(name, value)| (&**name, value))
    }

    pub fn operation_context_params(self) -> impl ExactSizeIterator<Item = (&'s str, &'s str)> {
        let params = self.data().operation_context_params.iter();
        params.map(|(name, path)| (&**name, &**path))
    }
}

/// The service, as smithy-rs reads the service shape.
#[derive(Clone, Copy)]
pub struct ServiceSchema<'s> {
    pub(super) set: &'s SchemaSet,
}

impl std::fmt::Debug for ServiceSchema<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ServiceSchema")
            .field(&self.sdk_id())
            .finish()
    }
}

impl<'s> ServiceSchema<'s> {
    fn data(self) -> &'s ServiceData {
        &self.set.service
    }

    pub fn version(self) -> Option<&'s str> {
        self.data().version.as_deref()
    }

    pub fn sdk_id(self) -> Option<&'s str> {
        self.data().sdk_id.as_deref()
    }

    pub fn endpoint_prefix(self) -> Option<&'s str> {
        self.data().endpoint_prefix.as_deref()
    }

    pub fn sigv4_name(self) -> Option<&'s str> {
        self.data().sigv4_name.as_deref()
    }

    pub fn auth(self) -> Option<impl ExactSizeIterator<Item = &'s str> + 's> {
        texts(self.data().auth.as_deref())
    }

    pub fn xml_namespace(self) -> Option<&'s str> {
        self.data().xml_namespace.as_deref()
    }

    pub fn query_compatible(self) -> bool {
        self.data().query_compatible
    }

    pub fn protocol_traits(self) -> &'s [TraitId] {
        &self.data().protocol_traits
    }

    pub fn client_context_params(self) -> &'s [ClientContextParam] {
        &self.data().client_context_params
    }

    /// The operations in model order (`ServiceModel.operation_names`).
    pub fn operations(self) -> impl ExactSizeIterator<Item = OperationSchema<'s>> + 's {
        let set = self.set;
        (0..set.operations.len()).map(move |index| OperationSchema {
            set,
            index: OperationIndex(index as u32),
        })
    }

    pub fn operation(self, name: &str) -> Option<OperationSchema<'s>> {
        self.operations().find(|operation| operation.name() == name)
    }
}
