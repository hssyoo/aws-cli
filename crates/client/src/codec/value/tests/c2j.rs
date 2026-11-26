//! A test-only C2J reader: the part of a `service-2.json` a JSON body's
//! walk reads (types, members, `locationName`, `timestampFormat`,
//! `document`, `union`), as a `SchemaSet`, until the C2J front-end
//! builds one for every model.
use super::Result;
use model::schema::{
    ColdTraits, DraftId, KnownTraits, MemberTraits, OperationData, OperationShapes, SchemaSet,
    SchemaSetBuilder, ShapeType,
};
use serde_json::Value as Json;
use std::collections::HashMap;

fn shape_type(shape: &Json) -> ShapeType {
    let flag = |key: &str| shape[key].as_bool() == Some(true);
    match shape["type"].as_str().unwrap_or_default() {
        "structure" if flag("document") => ShapeType::Document,
        "structure" if flag("union") => ShapeType::Union,
        "structure" => ShapeType::Structure,
        "list" => ShapeType::List,
        "map" => ShapeType::Map,
        "string" => ShapeType::String,
        "integer" => ShapeType::Integer,
        "long" => ShapeType::Long,
        "boolean" => ShapeType::Boolean,
        "float" => ShapeType::Float,
        "double" => ShapeType::Double,
        "timestamp" => ShapeType::Timestamp,
        "blob" => ShapeType::Blob,
        _ => ShapeType::Unknown,
    }
}

/// The slots a shape or a reference sets.
fn known(builder: &mut SchemaSetBuilder, json: &Json) -> Result<KnownTraits> {
    let mut text = |key: &str| -> Result<_> {
        json[key]
            .as_str()
            .map(|text| builder.intern(text))
            .transpose()
            .map_err(|error| format!("{error:?}").into())
    };
    Ok(KnownTraits {
        location: text("location")?,
        location_name: text("locationName")?,
        timestamp_format: text("timestampFormat")?,
        ..KnownTraits::default()
    })
}

struct Reader<'j> {
    shapes: &'j serde_json::Map<String, Json>,
    ids: HashMap<&'j str, DraftId>,
}

impl Reader<'_> {
    /// A reference's target and its traits merged over the target's.
    fn reference(
        &self,
        builder: &mut SchemaSetBuilder,
        reference: &Json,
    ) -> Result<(DraftId, MemberTraits)> {
        let name = reference["shape"]
            .as_str()
            .ok_or("a reference without shape")?;
        let target = *self.ids.get(name).ok_or("a reference to no shape")?;
        let own = known(builder, reference)?;
        if own == KnownTraits::default() {
            return Ok((target, MemberTraits::Inherit));
        }
        let under = known(builder, &self.shapes[name])?;
        let resolved = MemberTraits::resolved(under.overlaid(&own), ColdTraits::default());
        Ok((target, resolved))
    }
}

pub(super) fn schema_set(model: &Json) -> Result<SchemaSet> {
    let error = |error| format!("{error:?}");
    let mut builder = SchemaSetBuilder::new("c2j.test").map_err(error)?;
    let shapes = model["shapes"].as_object().ok_or("no shapes")?;
    let mut reader = Reader {
        shapes,
        ids: HashMap::new(),
    };
    for (name, shape) in shapes {
        let id = builder
            .declare(name, shape_type(shape), shape["type"].as_str())
            .map_err(error)?;
        let traits = known(&mut builder, shape)?;
        builder
            .set_traits(id, traits, ColdTraits::default())
            .map_err(error)?;
        reader.ids.insert(name, id);
    }
    for (name, shape) in shapes {
        let id = reader.ids[name.as_str()];
        for (member, reference) in shape["members"].as_object().into_iter().flatten() {
            let (target, traits) = reader.reference(&mut builder, reference)?;
            builder
                .add_member(id, member, target, traits)
                .map_err(error)?;
        }
        if let Some(member) = shape.get("member") {
            let (target, traits) = reader.reference(&mut builder, member)?;
            builder.set_list_member(id, target, traits).map_err(error)?;
        }
        if let (Some(key), Some(value)) = (shape.get("key"), shape.get("value")) {
            let key = reader.reference(&mut builder, key)?;
            let value = reader.reference(&mut builder, value)?;
            builder.set_map_entry(id, key, value).map_err(error)?;
        }
    }
    for (name, operation) in model["operations"].as_object().into_iter().flatten() {
        let mut root = |key: &str| -> Result<Option<DraftId>> {
            let Some(reference) = operation.get(key) else {
                return Ok(None);
            };
            let (target, traits) = reader.reference(&mut builder, reference)?;
            Ok(Some(builder.root_overlay(target, traits).map_err(error)?))
        };
        let shapes = OperationShapes {
            input: root("input")?,
            output: root("output")?,
            errors: Vec::new(),
        };
        let data = OperationData {
            name: name.as_str().into(),
            ..OperationData::default()
        };
        builder.add_operation(data, shapes).map_err(error)?;
    }
    Ok(builder.finish().map_err(error)?)
}
