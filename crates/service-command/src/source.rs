//! The registry's binder for the derived commands of a [`ModelSource`],
//! which supplies their tables: admission ([`crate::admit`]), then the
//! invocation.
use awscli_argparse::ParseResult;
use cli_core::cli_error::PythonReason;
use cli_core::client_config::user_agent::Versions;
use client::endpoint::EndpointData;
use client::hooks::Customization;
use command::models::ModelSource;
use command::{Admission, Binder, Bound, Needs};

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
    fn bind(&self, parsed: &ParseResult<'_>) -> Result<Bound, Admission> {
        let admitted = crate::admit::admit(&self.models, self.customizations, parsed)
            .map_err(Admission::Python)?;
        let service = self
            .models
            .service(&admitted.command)
            .ok_or(Admission::Python(PythonReason::UnsupportedSubcommand))?;
        let metadata = service.model.metadata();
        let needs = Needs {
            service: metadata
                .hyphenized_service_id()
                .unwrap_or_else(|| admitted.command.clone())
                .into(),
            reads_s3_settings: reads_s3_settings(&admitted.endpoints),
        };
        let invocation = crate::invoke::ServiceInvocation::new(
            admitted,
            std::sync::Arc::new(service.model.clone()),
            self.customizations,
            needs.service.clone().into_owned(),
            self.embedded_trust,
            self.versions,
        );
        Ok(Bound {
            invocation: Box::new(invocation),
            needs,
        })
    }
}

/// Whether a client with `endpoints` reads the `s3` block's settings: its
/// rule set takes an `AWS::S3::` or `AWS::S3Control::` built-in, which
/// botocore fills from them (`compute_endpoint_resolver_builtin_defaults`).
fn reads_s3_settings(endpoints: &EndpointData) -> bool {
    endpoints.parameters().iter().any(|parameter| {
        parameter
            .builtin
            .as_deref()
            .is_some_and(|builtin| builtin.starts_with("AWS::S3"))
    })
}

#[cfg(test)]
mod tests;
