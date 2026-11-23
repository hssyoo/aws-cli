//! The registry's model source: the derived commands of [`ModelSource`],
//! bound through [`crate::admit`].
use awscli_argparse::tables::RequiredOverride;
use awscli_argparse::{Argument, ParseResult};
use cli_core::cli_error::PythonReason;
use client::hooks::Customization;
use command::models::ModelSource;
use command::{Admission, Invocation, Needs, OperationSource};
use std::borrow::Cow;

/// Every model operation the search path's models define, admitted with
/// the client's customization table.
#[derive(Debug)]
pub struct ServiceSource<'a> {
    models: ModelSource<'a>,
    customizations: &'static [&'static Customization],
}

impl<'a> ServiceSource<'a> {
    pub const fn new(
        models: ModelSource<'a>,
        customizations: &'static [&'static Customization],
    ) -> Self {
        Self {
            models,
            customizations,
        }
    }
    pub const fn models(&self) -> &ModelSource<'a> {
        &self.models
    }
}

impl OperationSource for ServiceSource<'_> {
    fn is_root(&self, name: &str) -> bool {
        self.models.is_root(name)
    }
    fn operations(&self, root: &str) -> Option<Cow<'_, [&str]>> {
        self.models.operations(root)
    }
    fn arguments(&self, root: &str, operation: &str) -> Option<Cow<'_, [Argument<'_>]>> {
        self.models.arguments(root, operation)
    }
    fn overrides(&self, root: &str, operation: &str) -> Cow<'_, [RequiredOverride<'_>]> {
        self.models.overrides(root, operation)
    }
    fn reproduces_failures(&self, root: &str, operation: Option<&str>) -> bool {
        self.models.reproduces_failures(root, operation)
    }
    fn needs(&self, root: &str) -> Needs {
        self.models.needs(root)
    }
    /// Admission, then the call. The call loop is not integrated yet, so an
    /// admitted invocation is Python's (`ClientUnavailable`).
    fn bind(&self, parsed: &ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission> {
        crate::admit::admit(&self.models, self.customizations, parsed)
            .map_err(Admission::Python)
            .and_then(|_| Err(Admission::Python(PythonReason::ClientUnavailable)))
    }
}
