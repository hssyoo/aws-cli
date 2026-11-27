//! Model front-ends: each builds a [`crate::schema::SchemaSet`] from its own
//! source. [`c2j`] reads botocore's `service-2.json`; a test-only Smithy AST
//! fixture and, later, the Smithy JSON AST join it.
pub mod c2j;
