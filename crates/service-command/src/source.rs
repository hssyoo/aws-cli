//! The registry's binder for the derived commands of a [`ModelSource`],
//! which supplies their tables: admission ([`crate::admit`]), then the
//! invocation.
use awscli_argparse::ParseResult;
use cli_core::cli_error::PythonReason;
use cli_core::client_config::user_agent::Versions;
use client::endpoint::EndpointData;
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
    /// Admission, then the invocation, with the endpoint data its client
    /// loads as botocore's client creation does.
    fn bind(&self, parsed: &ParseResult<'_>) -> Result<Box<dyn Invocation>, Admission> {
        let admitted = crate::admit::admit(&self.models, self.customizations, parsed)
            .map_err(Admission::Python)?;
        let refused = || Admission::Python(PythonReason::UnsupportedTrait);
        let service = self.models.service(&admitted.command).ok_or_else(refused)?;
        let endpoints = EndpointData::load(
            self.models.search(),
            &service.name,
            service.model.metadata(),
        )
        .map_err(|_| refused())?;
        let service_id = self.models.needs(&admitted.command).service;
        Ok(Box::new(crate::invoke::ServiceInvocation::new(
            admitted,
            std::sync::Arc::new(service.model.clone()),
            endpoints,
            self.customizations,
            service_id.into_owned(),
            self.embedded_trust,
            self.versions,
        )))
    }
}
