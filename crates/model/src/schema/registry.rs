//! The error registry: botocore's error-shape lookups over the named range.
use super::known::Flag;
use super::{Schema, SchemaSet};

/// botocore's `ServiceModel.error_shapes` and `shape_for_error_code`, as
/// smithy-rs's `TypeRegistry`. It walks the named range only: never the
/// prelude, a member or a root overlay.
#[derive(Clone, Copy)]
pub struct TypeRegistry<'s> {
    pub(super) set: &'s SchemaSet,
}

impl std::fmt::Debug for TypeRegistry<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeRegistry").finish_non_exhaustive()
    }
}

impl<'s> TypeRegistry<'s> {
    /// `ServiceModel.error_shapes`: named shapes marked as errors, in model
    /// order.
    pub fn error_shapes(self) -> impl Iterator<Item = Schema<'s>> + 's {
        self.set.named().filter(|schema| schema.flag(Flag::Error))
    }

    /// `StructureShape.error_code`: `error.code` when non-empty, else the
    /// short name; only for an error shape.
    pub fn error_code(self, schema: Schema<'s>) -> Option<&'s str> {
        if !schema.flag(Flag::Error) {
            return None;
        }
        let code = schema.error().and_then(|error| error.code);
        Some(
            code.filter(|code| !code.is_empty())
                .unwrap_or(schema.id().name()),
        )
    }

    /// `ServiceModel.shape_for_error_code`: the last error shape in model
    /// order with the code.
    pub fn entry_for_error_code(self, code: &str) -> Option<Schema<'s>> {
        self.error_shapes()
            .filter(|schema| self.error_code(*schema) == Some(code))
            .last()
    }
}
