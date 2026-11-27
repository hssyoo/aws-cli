//! The C2J front-end: a [`SchemaSet`] from a parsed `service-2.json`, read
//! as botocore reads it.
//!
//! Its walk is the arena's, step for step, so a description both read
//! gives the same uninterpreted keys in the same order and fails with the
//! same [`ModelError`]: the document and `metadata`, `clientContextParams`,
//! each named shape's own keys, then each shape's members, list member and
//! map key and value, then the operations. A reference's keys are laid
//! over its target's before either is translated ([`traits`]); one that
//! changes no trait inherits the target's.
use crate::schema::{
    DraftId, MemberTraits, OperationShapes, SchemaSet, SchemaSetBuilder, ShapeType, Site,
    Uninterpreted,
};
use crate::shapes::ModelError;
use serde_json::{Map, Value as Json};
use std::collections::HashMap;

pub(crate) mod keys;
mod read;
mod traits;

#[cfg(test)]
mod tests;

use traits::C2jTraits;

type Object = Map<String, Json>;

/// `object.get(key)`. A short object, as a shape, reference or operation
/// is, is scanned: comparing a few keys costs less than hashing one.
pub(crate) fn get<'a>(object: &'a Object, key: &str) -> Option<&'a Json> {
    if object.len() > 8 {
        return object.get(key);
    }
    object
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value))
}

static EMPTY: std::sync::LazyLock<Object> = std::sync::LazyLock::new(Object::new);

/// The namespace of a C2J model's shape ids: `c2j.<service-dir>`.
pub fn namespace(service: &str) -> String {
    format!("{}.{service}", crate::schema::C2J_NAMESPACE)
}

/// A named shape as the walk holds it.
struct Named<'a> {
    id: DraftId,
    type_name: &'a str,
    shape_type: ShapeType,
    traits: C2jTraits<'a>,
}

struct Walk<'a> {
    builder: SchemaSetBuilder,
    definitions: &'a Object,
    index: HashMap<&'a str, usize>,
    named: Vec<Named<'a>>,
    uninterpreted: Vec<Uninterpreted>,
    /// A structure's resolved members, held until all resolve.
    pending: Vec<(&'a str, (DraftId, MemberTraits))>,
}

/// The schemas of `json` with its short names in `namespace`.
pub fn build(json: &Json, namespace: &str) -> Result<SchemaSet, ModelError> {
    let document = json.as_object().ok_or(ModelError::NotAnObject)?;
    let definitions = read::section(document, "shapes")?;
    let mut walk = Walk {
        builder: SchemaSetBuilder::new(namespace)?,
        definitions,
        index: definitions
            .keys()
            .enumerate()
            .map(|(i, name)| (name.as_str(), i))
            .collect(),
        named: Vec::with_capacity(definitions.len()),
        uninterpreted: Vec::new(),
        pending: Vec::new(),
    };
    let operations = document.get("operations").and_then(Json::as_object);
    walk.builder
        .reserve(definitions.len(), operations.map_or(0, Map::len));
    let at = || (Site::Service, String::new());
    let (known, ignored) = (keys::DOCUMENT, keys::DOCUMENT_IGNORED);
    keys::record(document, known, ignored, &at, &mut walk.uninterpreted);
    let mut service = read::metadata(document.get("metadata"), &mut walk.uninterpreted)?;
    service.client_context_params = read::client_context_params(document, &mut walk.uninterpreted)?;
    walk.builder.set_service(service);
    walk.named_shapes()?;
    walk.children()?;
    for (name, operation) in read::section(document, "operations")? {
        walk.operation(name, operation)?;
    }
    for Uninterpreted { site, key } in std::mem::take(&mut walk.uninterpreted) {
        walk.builder.record_uninterpreted(site, &key);
    }
    Ok(walk.builder.finish()?)
}

impl<'a> Walk<'a> {
    /// Every named shape with its own traits, in document order.
    fn named_shapes(&mut self) -> Result<(), ModelError> {
        for (name, definition) in self.definitions {
            let invalid = || ModelError::InvalidShape(name.as_str().into());
            let object = definition.as_object().ok_or_else(invalid)?;
            let type_name = get(object, "type").and_then(Json::as_str);
            let type_name = type_name.ok_or_else(invalid)?;
            let mut traits = C2jTraits::default();
            traits.apply(object, name, &mut self.builder)?;
            let at = || (Site::Shape(name.as_str().into()), String::new());
            let (known, ignored) = (keys::SHAPE, keys::SHAPE_IGNORED);
            keys::record_traits(object, known, ignored, &at, &mut self.uninterpreted);
            let shape_type = traits.shape_type(type_name);
            let id = self.builder.declare(name, shape_type, Some(type_name))?;
            let cold = traits.cold(shape_type)?;
            self.builder.set_traits(id, traits.known.clone(), cold)?;
            self.named.push(Named {
                id,
                type_name,
                shape_type,
                traits,
            });
        }
        Ok(())
    }

    /// The members, list member and map key and value of each named shape.
    fn children(&mut self) -> Result<(), ModelError> {
        for (index, (name, definition)) in self.definitions.iter().enumerate() {
            let object = definition.as_object().unwrap_or(&EMPTY);
            let parent = self.named[index].id;
            match self.named[index].type_name {
                "structure" => self.structure(parent, name, object)?,
                "list" => {
                    let member = self.child(name, object, "member")?;
                    self.builder.set_list_member(parent, member.0, member.1)?;
                }
                "map" => {
                    let key = self.child(name, object, "key")?;
                    let value = self.child(name, object, "value")?;
                    self.builder.set_map_entry(parent, key, value)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn structure(
        &mut self,
        parent: DraftId,
        name: &str,
        object: &'a Object,
    ) -> Result<(), ModelError> {
        let definitions = match get(object, "members") {
            None => &*EMPTY,
            Some(members) => members
                .as_object()
                .ok_or_else(|| ModelError::InvalidShape(name.into()))?,
        };
        let mut resolved = std::mem::take(&mut self.pending);
        resolved.clear();
        for (member, reference) in definitions {
            let at = || (Site::Shape(name.into()), format!("members.{member}."));
            resolved.push((member, self.reference(name, reference, &at)?));
        }
        self.builder.reserve_members(parent, resolved.len());
        for (member, (target, traits)) in resolved.drain(..) {
            self.builder.add_member(parent, member, target, traits)?;
        }
        self.pending = resolved;
        Ok(())
    }

    fn child(
        &mut self,
        name: &str,
        object: &Object,
        key: &str,
    ) -> Result<(DraftId, MemberTraits), ModelError> {
        let reference = get(object, key).ok_or_else(|| ModelError::InvalidShape(name.into()))?;
        self.reference(name, reference, &|| {
            (Site::Shape(name.into()), format!("{key}."))
        })
    }

    /// `ShapeResolver.resolve_shape_ref`, in the shape or operation `owner`:
    /// the target and the reference's resolved traits, recording at `at`
    /// the keys no reader interprets and a `document` or `union` that
    /// changes what a structure target is.
    fn reference(
        &mut self,
        owner: &str,
        reference: &Json,
        at: keys::At<'_>,
    ) -> Result<(DraftId, MemberTraits), ModelError> {
        let invalid = || ModelError::InvalidShapeReference(owner.into());
        let object = reference.as_object().ok_or_else(invalid)?;
        let (shape, has_traits) = keys::scan_reference(object, at, &mut self.uninterpreted);
        let target_name = shape.and_then(Json::as_str).ok_or_else(invalid)?;
        let index = *self
            .index
            .get(target_name)
            .ok_or_else(|| ModelError::NoShapeFound(target_name.into()))?;
        let target = &self.named[index];
        let id = target.id;
        if !has_traits {
            return Ok((id, MemberTraits::Inherit));
        }
        let mut traits = target.traits.clone();
        traits.apply(object, target_name, &mut self.builder)?;
        let target = &self.named[index];
        if traits == target.traits {
            return Ok((id, MemberTraits::Inherit));
        }
        if target.type_name == "structure" {
            for key in traits.changed_kind(&target.traits) {
                let (site, prefix) = at();
                let key = format!("{prefix}{key}").into();
                self.uninterpreted.push(Uninterpreted { site, key });
            }
        }
        let cold = traits.cold(target.shape_type)?;
        Ok((id, MemberTraits::resolved(traits.known, cold)))
    }

    /// An operation reference's root: the target, or an overlay of it.
    fn root(
        &mut self,
        owner: &str,
        reference: &Json,
        at: keys::At<'_>,
    ) -> Result<DraftId, ModelError> {
        let (target, traits) = self.reference(owner, reference, at)?;
        Ok(self.builder.root_overlay(target, traits)?)
    }

    fn operation(&mut self, name: &str, operation: &Json) -> Result<(), ModelError> {
        let object = operation.as_object().unwrap_or(&EMPTY);
        keys::record_operation(name, object, &mut self.uninterpreted);
        let mut shapes = OperationShapes::default();
        for (key, slot) in [("input", &mut shapes.input), ("output", &mut shapes.output)] {
            if let Some(reference) = get(object, key) {
                let at = || (Site::Operation(name.into()), format!("{key}."));
                *slot = Some(self.root(name, reference, &at)?);
            }
        }
        if let Some(errors) = get(object, "errors") {
            let errors = errors.as_array().ok_or_else(|| ModelError::InvalidTrait {
                shape: name.into(),
                name: "errors",
            })?;
            for error in errors {
                let at = || (Site::Operation(name.into()), "errors.".to_owned());
                let root = self.root(name, error, &at)?;
                shapes.errors.push(root);
            }
        }
        let data = read::operation(name, object)?;
        self.builder.add_operation(data, shapes)?;
        Ok(())
    }
}
