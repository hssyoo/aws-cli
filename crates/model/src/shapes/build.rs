//! Builds a [`Model`] from a service description in three passes: named
//! shapes and their own traits, then their children, then the kinds of the
//! shapes references derive.
use super::traits::{has_trait_keys, strings, truthy};
use super::*;
use serde_json::{Map as JsonMap, Value as Json};

type Object = JsonMap<String, Json>;

/// A shape id from an arena index the builder checked.
pub(super) fn id(index: usize) -> ShapeId {
    ShapeId(u32::try_from(index).unwrap_or(u32::MAX))
}

static EMPTY: std::sync::LazyLock<Object> = std::sync::LazyLock::new(Object::new);

struct Builder<'a> {
    definitions: &'a Object,
    /// Named shape indexes by name.
    index: std::collections::HashMap<&'a str, usize>,
    shapes: Vec<Shape>,
    members: Vec<Member>,
    /// Shapes derived from a reference, with the named shape they copy.
    derived: Vec<(ShapeId, ShapeId)>,
}

pub(super) fn build(json: &Json) -> Result<Model, ModelError> {
    let document = json.as_object().ok_or(ModelError::NotAnObject)?;
    let definitions = section(document, "shapes")?;
    let mut builder = Builder {
        definitions,
        index: definitions
            .keys()
            .enumerate()
            .map(|(i, name)| (name.as_str(), i))
            .collect(),
        shapes: Vec::with_capacity(definitions.len()),
        members: Vec::new(),
        derived: Vec::new(),
    };
    builder.named_shapes()?;
    builder.children()?;
    let operations = section(document, "operations")?
        .iter()
        .map(|(name, operation)| builder.operation(name, operation))
        .collect::<Result<_, _>>()?;
    builder.derived_kinds();
    let named = definitions.len();
    let mut by_name: Vec<ShapeId> = (0..named).map(id).collect();
    by_name.sort_by(|a, b| {
        builder.shapes[a.index()]
            .name
            .cmp(&builder.shapes[b.index()].name)
    });
    Ok(Model {
        metadata: metadata(document.get("metadata"))?,
        client_context_params: client_context_params(document)?,
        operations,
        shapes: builder.shapes.into(),
        members: builder.members.into(),
        named,
        by_name: by_name.into(),
    })
}

/// A top-level object; absent reads as empty, as `ServiceModel` reads it.
fn section<'a>(document: &'a Object, key: &'static str) -> Result<&'a Object, ModelError> {
    match document.get(key) {
        None => Ok(&EMPTY),
        Some(value) => value.as_object().ok_or(ModelError::InvalidTrait {
            shape: "(model)".into(),
            name: key,
        }),
    }
}

impl Builder<'_> {
    /// Pass 1: every named shape with its own traits, compound kinds empty.
    fn named_shapes(&mut self) -> Result<(), ModelError> {
        if u32::try_from(self.definitions.len()).is_err() {
            return Err(ModelError::TooLarge);
        }
        for (name, definition) in self.definitions {
            let object = definition
                .as_object()
                .ok_or_else(|| ModelError::InvalidShape(name.as_str().into()))?;
            let type_name = object
                .get("type")
                .and_then(Json::as_str)
                .ok_or_else(|| ModelError::InvalidShape(name.as_str().into()))?;
            let mut traits = Traits::default();
            traits.apply(object, name)?;
            self.shapes.push(Shape {
                name: name.as_str().into(),
                kind: placeholder(type_name),
                traits,
            });
        }
        Ok(())
    }

    /// Pass 2: the members, list member and map key and value of each named
    /// shape.
    fn children(&mut self) -> Result<(), ModelError> {
        for (index, (name, definition)) in self.definitions.iter().enumerate() {
            let object = definition.as_object().unwrap_or(&EMPTY);
            let kind = match self.shapes[index].kind {
                ShapeKind::Structure { .. } => self.structure(name, object)?,
                ShapeKind::List { .. } => ShapeKind::List {
                    member: self.child(name, object, "member")?,
                },
                ShapeKind::Map { .. } => ShapeKind::Map {
                    key: self.child(name, object, "key")?,
                    value: self.child(name, object, "value")?,
                },
                _ => continue,
            };
            self.shapes[index].kind = kind;
        }
        Ok(())
    }

    fn structure(&mut self, name: &str, object: &Object) -> Result<ShapeKind, ModelError> {
        let definitions = match object.get("members") {
            None => &*EMPTY,
            Some(members) => members
                .as_object()
                .ok_or_else(|| ModelError::InvalidShape(name.into()))?,
        };
        let mut resolved = Vec::with_capacity(definitions.len());
        for (member, reference) in definitions {
            resolved.push(Member {
                name: member.as_str().into(),
                shape: self.reference(name, reference)?,
            });
        }
        let start = u32::try_from(self.members.len()).map_err(|_| ModelError::TooLarge)?;
        let len = u32::try_from(resolved.len()).map_err(|_| ModelError::TooLarge)?;
        self.members.extend(resolved);
        Ok(ShapeKind::Structure {
            members: Members { start, len },
        })
    }

    fn child(&mut self, name: &str, object: &Object, key: &str) -> Result<ShapeId, ModelError> {
        let reference = object
            .get(key)
            .ok_or_else(|| ModelError::InvalidShape(name.into()))?;
        self.reference(name, reference)
    }

    /// `ShapeResolver.resolve_shape_ref`, in the shape or operation `owner`.
    fn reference(&mut self, owner: &str, reference: &Json) -> Result<ShapeId, ModelError> {
        let invalid = || ModelError::InvalidShapeReference(owner.into());
        let object = reference.as_object().ok_or_else(invalid)?;
        let target_name = object
            .get("shape")
            .and_then(Json::as_str)
            .ok_or_else(invalid)?;
        let target = self
            .index
            .get(target_name)
            .map(|index| id(*index))
            .ok_or_else(|| ModelError::NoShapeFound(target_name.into()))?;
        if !has_trait_keys(object) {
            return Ok(target);
        }
        let shape = &self.shapes[target.index()];
        let mut traits = shape.traits.clone();
        traits.apply(object, &shape.name)?;
        if traits == shape.traits {
            return Ok(target);
        }
        let derived = id(self.shapes.len());
        if u32::try_from(self.shapes.len()).is_err() {
            return Err(ModelError::TooLarge);
        }
        self.shapes.push(Shape {
            name: shape.name.clone(),
            kind: ShapeKind::Boolean,
            traits,
        });
        self.derived.push((derived, target));
        Ok(derived)
    }

    fn optional_reference(
        &mut self,
        owner: &str,
        reference: Option<&Json>,
    ) -> Result<Option<ShapeId>, ModelError> {
        reference
            .map(|reference| self.reference(owner, reference))
            .transpose()
    }

    /// Pass 3: a derived shape has its target's kind and children.
    fn derived_kinds(&mut self) {
        for (derived, target) in std::mem::take(&mut self.derived) {
            self.shapes[derived.index()].kind = self.shapes[target.index()].kind.clone();
        }
    }

    fn operation(&mut self, name: &str, operation: &Json) -> Result<Operation, ModelError> {
        let object = operation.as_object().unwrap_or(&EMPTY);
        let read = Fields {
            object,
            owner: name,
        };
        let input = self.optional_reference(name, object.get("input"))?;
        let output = self.optional_reference(name, object.get("output"))?;
        let errors = match object.get("errors") {
            None => Box::default(),
            Some(errors) => errors
                .as_array()
                .ok_or_else(|| read.invalid("errors"))?
                .iter()
                .map(|error| self.reference(name, error))
                .collect::<Result<_, _>>()?,
        };
        Ok(Operation {
            name: name.into(),
            wire_name: read.text("name")?,
            http: read.http()?,
            input,
            output,
            errors,
            deprecated: read.flag("deprecated"),
            endpoint_discovery: read.endpoint_discovery()?,
            endpoint_operation: read.flag("endpointoperation"),
            auth: read.texts("auth")?,
            auth_type: read.text("authtype")?,
            unsigned_payload: read.flag("unsignedPayload"),
            http_checksum_required: read.flag("httpChecksumRequired"),
            http_checksum: read.http_checksum()?,
            request_compression: read.request_compression()?,
            static_context_params: read.static_context_params()?,
            operation_context_params: read.operation_context_params()?,
            host_prefix: read.host_prefix()?,
        })
    }
}

/// A named shape's kind before its children are resolved.
fn placeholder(type_name: &str) -> ShapeKind {
    match type_name {
        "structure" => ShapeKind::Structure {
            members: Members::default(),
        },
        "list" => ShapeKind::List { member: id(0) },
        "map" => ShapeKind::Map {
            key: id(0),
            value: id(0),
        },
        "string" => ShapeKind::String,
        "integer" => ShapeKind::Integer,
        "long" => ShapeKind::Long,
        "boolean" => ShapeKind::Boolean,
        "float" => ShapeKind::Float,
        "double" => ShapeKind::Double,
        "timestamp" => ShapeKind::Timestamp,
        "blob" => ShapeKind::Blob,
        other => ShapeKind::Other(other.into()),
    }
}

/// Typed reads of an operation's or the metadata's keys.
struct Fields<'a> {
    object: &'a Object,
    owner: &'a str,
}

impl Fields<'_> {
    fn invalid(&self, name: &'static str) -> ModelError {
        ModelError::InvalidTrait {
            shape: self.owner.into(),
            name,
        }
    }

    fn flag(&self, key: &str) -> bool {
        self.object.get(key).is_some_and(truthy)
    }

    fn text(&self, key: &'static str) -> Result<Option<Box<str>>, ModelError> {
        self.object
            .get(key)
            .map(|value| {
                value
                    .as_str()
                    .map(Box::from)
                    .ok_or_else(|| self.invalid(key))
            })
            .transpose()
    }

    fn texts(&self, key: &'static str) -> Result<Option<Box<[Box<str>]>>, ModelError> {
        self.object
            .get(key)
            .map(|value| strings(value).ok_or_else(|| self.invalid(key)))
            .transpose()
    }

    fn nested(&self, key: &'static str) -> Result<Option<Fields<'_>>, ModelError> {
        let Some(value) = self.object.get(key) else {
            return Ok(None);
        };
        let object = value.as_object().ok_or_else(|| self.invalid(key))?;
        Ok(Some(Fields {
            object,
            owner: self.owner,
        }))
    }

    fn http(&self) -> Result<Http, ModelError> {
        let Some(http) = self.nested("http")? else {
            return Ok(Http::default());
        };
        let code = http.object.get("responseCode").and_then(Json::as_u64);
        Ok(Http {
            method: http.text("method")?,
            request_uri: http.text("requestUri")?,
            response_code: code.and_then(|code| u16::try_from(code).ok()),
        })
    }

    fn endpoint_discovery(&self) -> Result<Option<EndpointDiscovery>, ModelError> {
        Ok(self
            .nested("endpointdiscovery")?
            .map(|discovery| EndpointDiscovery {
                required: discovery.flag("required"),
            }))
    }

    fn http_checksum(&self) -> Result<Option<HttpChecksum>, ModelError> {
        let Some(checksum) = self.nested("httpChecksum")? else {
            return Ok(None);
        };
        Ok(Some(HttpChecksum {
            request_algorithm_member: checksum.text("requestAlgorithmMember")?,
            request_checksum_required: checksum.flag("requestChecksumRequired"),
            request_validation_mode_member: checksum.text("requestValidationModeMember")?,
            response_algorithms: checksum.texts("responseAlgorithms")?.unwrap_or_default(),
        }))
    }

    fn request_compression(&self) -> Result<Option<Box<[Box<str>]>>, ModelError> {
        let Some(compression) = self.nested("requestcompression")? else {
            return Ok(None);
        };
        Ok(Some(compression.texts("encodings")?.unwrap_or_default()))
    }

    fn static_context_params(&self) -> Result<StaticContextParams, ModelError> {
        let Some(params) = self.nested("staticContextParams")? else {
            return Ok(Box::default());
        };
        let value = |props: &Json| props.get("value").map_or(Value::Null, Value::from_serde);
        Ok(params
            .object
            .iter()
            .map(|(name, props)| (name.as_str().into(), value(props)))
            .collect())
    }

    fn operation_context_params(&self) -> Result<OperationContextParams, ModelError> {
        let Some(params) = self.nested("operationContextParams")? else {
            return Ok(Box::default());
        };
        params
            .object
            .iter()
            .map(|(name, props)| {
                let path = props.get("path").and_then(Json::as_str);
                let path = path.ok_or_else(|| self.invalid("operationContextParams"))?;
                Ok((name.as_str().into(), path.into()))
            })
            .collect()
    }

    fn host_prefix(&self) -> Result<Option<Box<str>>, ModelError> {
        match self.nested("endpoint")? {
            Some(endpoint) => endpoint.text("hostPrefix"),
            None => Ok(None),
        }
    }
}

fn metadata(metadata: Option<&Json>) -> Result<Metadata, ModelError> {
    let object = match metadata {
        None => &*EMPTY,
        Some(value) => value.as_object().ok_or(ModelError::InvalidTrait {
            shape: "(model)".into(),
            name: "metadata",
        })?,
    };
    let read = Fields {
        object,
        owner: "metadata",
    };
    Ok(Metadata {
        api_version: read.text("apiVersion")?,
        endpoint_prefix: read.text("endpointPrefix")?,
        protocol: read.text("protocol")?,
        protocols: read.texts("protocols")?,
        json_version: read.text("jsonVersion")?,
        target_prefix: read.text("targetPrefix")?,
        service_id: read.text("serviceId")?,
        service_full_name: read.text("serviceFullName")?,
        service_abbreviation: read.text("serviceAbbreviation")?,
        signature_version: read.text("signatureVersion")?,
        signing_name: read.text("signingName")?,
        uid: read.text("uid")?,
        xml_namespace: read.text("xmlNamespace")?,
        global_endpoint: read.text("globalEndpoint")?,
        checksum_format: read.text("checksumFormat")?,
        auth: read.texts("auth")?,
        aws_query_compatible: object.contains_key("awsQueryCompatible"),
    })
}

/// `ServiceModel.client_context_parameters`.
fn client_context_params(document: &Object) -> Result<Box<[ClientContextParam]>, ModelError> {
    let params = section(document, "clientContextParams")?;
    params
        .iter()
        .map(|(name, param)| {
            let type_name = param.get("type").and_then(Json::as_str);
            let type_name = type_name.ok_or(ModelError::InvalidTrait {
                shape: name.as_str().into(),
                name: "clientContextParams",
            })?;
            Ok(ClientContextParam {
                name: name.as_str().into(),
                type_name: type_name.into(),
            })
        })
        .collect()
}
