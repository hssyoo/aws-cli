//! The schema an arena [`ShapeId`] names: the bridge the validator read
//! through until its signatures took [`Schema`]s, kept for the test that
//! checks every arena shape finds a schema that reads the same. It goes
//! with the arena.
use crate::schema::Schema;
use crate::shapes::{Model, ShapeId, ShapeKind};

/// The schema the C2J front-end built for the arena's `shape`: a named
/// shape by its name, an operation reference by its operation, and a
/// member, list member or map key or value by its container.
pub(super) fn schema_for(model: &Model, shape: ShapeId) -> Option<Schema<'_>> {
    let set = model.schema();
    let name = &*model.shape(shape).name;
    if model.shape_named(name) == Some(shape) {
        return set.shape_named(name);
    }
    let service = set.service();
    for operation in model.operations() {
        let Some(schema) = service.operation(&operation.name) else {
            continue;
        };
        if operation.input == Some(shape) {
            return schema.input();
        }
        if operation.output == Some(shape) {
            return schema.output();
        }
        let mut errors = operation.errors.iter().zip(schema.errors());
        if let Some((_, error)) = errors.find(|(id, _)| **id == shape) {
            return Some(error);
        }
    }
    model
        .named_shapes()
        .find_map(|named| child(model, named, shape))
}

/// `shape` as a child of the named shape `named`.
pub(super) fn child(model: &Model, named: ShapeId, shape: ShapeId) -> Option<Schema<'_>> {
    let container = model.schema().shape_named(&model.shape(named).name)?;
    match &model.shape(named).kind {
        ShapeKind::Structure { .. } => model
            .members(named)
            .iter()
            .zip(container.members())
            .find_map(|(member, schema)| (member.shape == shape).then_some(schema)),
        ShapeKind::List { member } => (*member == shape)
            .then(|| container.list_member())
            .flatten(),
        ShapeKind::Map { key, .. } if *key == shape => container.map_key(),
        ShapeKind::Map { value, .. } if *value == shape => container.map_value(),
        _ => None,
    }
}
