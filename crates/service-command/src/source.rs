//! The registry's model source: the derived commands of [`ModelSource`],
//! bound through [`crate::admit`].
use awscli_argparse::tables::RequiredOverride;
use awscli_argparse::{Argument, ParseResult};
use cli_core::cli_error::PythonReason;
use cli_core::client_config::user_agent::Versions;
use client::endpoint::EndpointData;
use client::hooks::Customization;
use command::models::ModelSource;
use command::{Admission, Invocation, Needs, OperationSource};
use std::borrow::Cow;

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
        let Needs::Client {
            service: service_id,
        } = self.models.needs(&admitted.command);
        let namespace = &parsed.globals.namespace;
        Ok(Box::new(crate::invoke::ServiceInvocation::new(
            admitted,
            std::sync::Arc::new(service.model.clone()),
            endpoints,
            self.customizations,
            service_id.into_owned(),
            namespace.string("output").map(str::to_owned),
            namespace.boolean("no_cli_pager"),
            self.embedded_trust,
            self.versions,
        )))
    }
}
