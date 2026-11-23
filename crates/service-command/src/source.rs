//! The registry's binder for the derived commands of a [`ModelSource`],
//! which supplies their tables: admission ([`crate::admit`]), then the
//! invocation.
use awscli_argparse::ParseResult;
use cli_core::cli_error::PythonReason;
use cli_core::client_config::user_agent::Versions;
use client::hooks::Customization;
use command::models::ModelSource;
use command::{Admission, Binder, Invocation, OperationSource};

/// Every model operation the search path's models define, admitted with
/// the client's customization table, its clients trusting the payload's CA
/// bundle when nothing configures another.
#[derive(Debug)]
pub struct ServiceSource<'a> {
    models: ModelSource<'a>,
    customizations: &'static [&'static Customization],
    embedded_trust: Option<&'static [u8]>,
    versions: Versions<'static>,
}

impl<'a> ServiceSource<'a> {
    pub const fn new(
        models: ModelSource<'a>,
        customizations: &'static [&'static Customization],
        embedded_trust: Option<&'static [u8]>,
        versions: Versions<'static>,
    ) -> Self {
        Self {
            models,
            customizations,
            embedded_trust,
            versions,
        }
    }
    pub const fn models(&self) -> &ModelSource<'a> {
        &self.models
    }
}

impl Binder for ServiceSource<'_> {
    /// Admission, then the invocation.
    fn bind(&self, parsed: &ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission> {
        let admitted = crate::admit::admit(&self.models, self.customizations, parsed)
            .map_err(Admission::Python)?;
        let service = self
            .models
            .service(&admitted.command)
            .ok_or(Admission::Python(PythonReason::UnsupportedSubcommand))?;
        let service_id = self.models.needs(&admitted.command).service;
        Ok(Box::new(crate::invoke::ServiceInvocation::new(
            admitted,
            std::sync::Arc::new(service.model.clone()),
            self.customizations,
            service_id.into_owned(),
            self.embedded_trust,
            self.versions,
        )))
    }
}
