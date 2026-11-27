//! The registry's binder for the derived commands of a [`ModelSource`],
//! which supplies their tables: admission ([`crate::admit`]), then the
//! invocation.
use crate::admit::Outcome;
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
    /// The paging handlers' (`bind::precedes_unpack_failure`).
    fn precedes_unpack_failure(&self, parsed: &ParseResult<'_>) -> bool {
        let [command, operation] = parsed.lineage.as_slice() else {
            return false;
        };
        self.models
            .table(&command.name, &operation.name)
            .is_some_and(|table| crate::bind::precedes_unpack_failure(table, parsed))
    }
    /// Admission, then the invocation: the call's, or one that reports what
    /// a paging handler raised. That one makes no client, but the router
    /// judges the client settings before any invocation runs, so a setting
    /// the native route cannot honour (a non-integer `max_attempts`, a
    /// missing `ca_bundle`, an unknown `s3.addressing_style`) still routes
    /// the command first: to Python, which reports the paging error, so the
    /// output is the same, or, on a run forcing the native route, to a
    /// refusal.
    fn bind(&self, parsed: &ParseResult<'_>) -> Result<Bound, Admission> {
        let outcome = crate::admit::admit(&self.models, self.customizations, parsed)
            .map_err(Admission::Python)?;
        let command = match &outcome {
            Outcome::Call(admitted) => &admitted.command,
            Outcome::Raised { command, .. } => command,
        };
        let service = self
            .models
            .service(command)
            .ok_or(Admission::Python(PythonReason::UnsupportedSubcommand))?;
        let service_id = service
            .model
            .facts()
            .hyphenized_service_id()
            .unwrap_or_else(|| command.clone());
        let admitted = match outcome {
            Outcome::Call(admitted) => *admitted,
            Outcome::Raised { failure, .. } => {
                return Ok(Bound {
                    invocation: Box::new(crate::invoke::Raised(failure)),
                    needs: Needs {
                        service: service_id.into(),
                        reads_s3_settings: false,
                    },
                });
            }
        };
        let needs = Needs {
            service: service_id.into(),
            reads_s3_settings: reads_s3_settings(&admitted.endpoints),
        };
        let invocation = crate::invoke::ServiceInvocation::new(
            admitted,
            std::sync::Arc::clone(&service.model),
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
