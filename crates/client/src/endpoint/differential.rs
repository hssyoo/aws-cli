//! A test-only second resolver: each case resolved both by the CRT's rules
//! engine, as [`super::resolve`] does today, and by the native engine
//! (`endpoint-rules`), over the same provider parameters. Every case must
//! give the same thing both ways, at two levels:
//!
//! - the engines' own results ([`Resolution`]): the url, the properties as
//!   canonical JSON (keys sorted; arrays keep their order), the header
//!   count, a rule's error text, or the engine failure's text;
//! - the client's: [`super::resolve`]'s `ResolvedEndpoint` or
//!   `CallError`, against the same mapping over the native result.
//!
//! A case where they differ is a [`Finding`]; nothing is skipped or
//! accepted here. The case sources are in `tests.rs`.
use super::{EndpointData, EndpointParams, ParamValue, ResolvedEndpoint, Unsupported};
use crate::error::CallError;
use endpoint_rules::{DecodeError, EvalValue, Library, Outcome, Params, Partitions, Program};
use std::fmt;

/// What one engine gives one case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Resolution {
    Endpoint {
        url: String,
        /// Canonical JSON, `{}` for none; the text as given when it is not
        /// JSON.
        properties: String,
        headers: usize,
    },
    /// A rule's error message, as the rule set writes it.
    RuleError(String),
    /// The engine failed: the CRT's `ResolveError` text, or the native
    /// error's display (the same four texts) with its detail.
    Failed { text: String, detail: String },
}

impl Resolution {
    /// Equal but for a failure's detail, which only the native engine has.
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Failed { text: a, .. }, Self::Failed { text: b, .. }) => a == b,
            (a, b) => a == b,
        }
    }
}

/// A service's rule set and partitions, decoded once for the native engine.
pub(super) struct Native {
    program: Result<Program, DecodeError>,
    partitions: Result<Partitions, DecodeError>,
}

impl Native {
    pub(super) fn new(data: &EndpointData) -> Self {
        Self {
            program: Program::decode(&data.rules),
            partitions: Partitions::decode(&data.partitions),
        }
    }
    /// The decode error, if either document failed.
    pub(super) fn decode_error(&self) -> Option<&DecodeError> {
        self.program
            .as_ref()
            .err()
            .or_else(|| self.partitions.as_ref().err())
    }
    /// The native engine's resolution of `params`, each bound by name to
    /// its parameter's slot; a name the rule set does not declare is
    /// dropped, as the CRT's context ignores it.
    pub(super) fn resolve(&self, params: &EndpointParams) -> Resolution {
        let failed = |error: &dyn fmt::Display, detail: &str| Resolution::Failed {
            text: error.to_string(),
            detail: detail.to_owned(),
        };
        // The CRT parses the rule set before the partitions.
        let program = match &self.program {
            Ok(program) => program,
            Err(error) => return failed(error, error.detail()),
        };
        let partitions = match &self.partitions {
            Ok(partitions) => partitions,
            Err(error) => return failed(error, error.detail()),
        };
        let mut args = Params::new(program);
        for (name, value) in params.iter() {
            if let Some(def) = program.param(name) {
                args.set(def.slot(), eval_value(value));
            }
        }
        match program.evaluate(&args, &Library { partitions }) {
            Ok(Outcome::Endpoint(endpoint)) => Resolution::Endpoint {
                url: endpoint.url,
                properties: canonical(&endpoint.properties.to_string()),
                headers: endpoint.headers.len(),
            },
            Ok(Outcome::Error(message)) => Resolution::RuleError(message),
            Err(error) => failed(&error, error.detail()),
        }
    }
    /// [`super::resolve`] over the native engine: the same refusal of
    /// headers, the same `authSchemes` reading and the same error texts.
    pub(super) fn client_resolve(
        &self,
        params: &EndpointParams,
    ) -> Result<ResolvedEndpoint, CallError> {
        match self.resolve(params) {
            Resolution::Endpoint { headers, .. } if headers > 0 => {
                Err(CallError::Unsupported(Unsupported::EndpointHeaders))
            }
            Resolution::Endpoint {
                url, properties, ..
            } => Ok(ResolvedEndpoint {
                url,
                auth_schemes: super::auth_schemes(properties.as_bytes()),
            }),
            Resolution::RuleError(message) => {
                Err(CallError::Endpoint(super::rule_error_text(&message)))
            }
            Resolution::Failed { text, .. } => Err(CallError::Endpoint(text)),
        }
    }
}

fn eval_value(value: &ParamValue) -> EvalValue {
    match value {
        ParamValue::String(text) => EvalValue::str(text),
        ParamValue::Boolean(flag) => EvalValue::Bool(*flag),
        ParamValue::StringArray(items) => {
            EvalValue::Array(items.iter().map(|item| EvalValue::str(item)).collect())
        }
    }
}

/// The CRT engine's resolution of `params`, by the same conversion
/// [`super::resolve`] makes.
pub(super) fn crt_resolve(data: &EndpointData, params: &EndpointParams) -> Resolution {
    use crt::endpoints::{Documents, ParamValue as Crt, Resolved};
    let arrays: Vec<Vec<&str>> = params
        .iter()
        .map(|(_, value)| match value {
            ParamValue::StringArray(items) => items.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        })
        .collect();
    let list: Vec<(&str, Crt<'_>)> = params
        .iter()
        .zip(&arrays)
        .map(|((name, value), array)| {
            let value = match value {
                ParamValue::String(text) => Crt::String(text),
                ParamValue::Boolean(flag) => Crt::Boolean(*flag),
                ParamValue::StringArray(_) => Crt::StringArray(array),
            };
            (name, value)
        })
        .collect();
    let documents = Documents {
        rules_json: &data.rules,
        partitions_json: &data.partitions,
    };
    let lossy = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    match crt::endpoints::resolve_with_headers(documents, &list) {
        Ok((Resolved::Endpoint { url, properties }, headers)) => Resolution::Endpoint {
            url: lossy(&url),
            // The CRT gives no bytes for an endpoint without properties.
            properties: if properties.is_empty() {
                "{}".to_owned()
            } else {
                canonical(&lossy(&properties))
            },
            headers,
        },
        Ok((Resolved::RuleError(message), _)) => Resolution::RuleError(lossy(&message)),
        Err(error) => Resolution::Failed {
            text: error.to_string(),
            detail: String::new(),
        },
    }
}

/// `text` as JSON with every object's keys sorted, and numbers as written;
/// `text` itself when it is not JSON.
pub(super) fn canonical(text: &str) -> String {
    fn sorted(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<_> = map.into_iter().collect();
                entries.sort_by(|(a, _), (b, _)| a.cmp(b));
                serde_json::Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key, sorted(value)))
                        .collect(),
                )
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.into_iter().map(sorted).collect())
            }
            other => other,
        }
    }
    serde_json::from_str(text).map_or_else(|_| text.to_owned(), |value| sorted(value).to_string())
}

/// A case the two engines resolve differently.
#[derive(Debug)]
pub(super) struct Finding {
    pub case: String,
    pub params: EndpointParams,
    pub crt: Resolution,
    pub native: Resolution,
    pub crt_client: Result<ResolvedEndpoint, CallError>,
    pub native_client: Result<ResolvedEndpoint, CallError>,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.case)?;
        writeln!(f, "  params  {:?}", self.params)?;
        if !self.crt.same(&self.native) {
            writeln!(f, "  crt     {:?}", self.crt)?;
            writeln!(f, "  native  {:?}", self.native)?;
        }
        if self.crt_client != self.native_client {
            writeln!(f, "  client crt     {:?}", self.crt_client)?;
            writeln!(f, "  client native  {:?}", self.native_client)?;
        }
        Ok(())
    }
}

/// Resolves `params` both ways; `None` when the engines and the client's
/// results agree.
pub(super) fn compare(
    case: impl FnOnce() -> String,
    data: &EndpointData,
    native: &Native,
    params: &EndpointParams,
) -> Option<Finding> {
    let crt = crt_resolve(data, params);
    let ours = native.resolve(params);
    let crt_client = super::resolve(data, params);
    let native_client = native.client_resolve(params);
    (!crt.same(&ours) || crt_client != native_client).then(|| Finding {
        case: case(),
        params: params.clone(),
        crt,
        native: ours,
        crt_client,
        native_client,
    })
}

#[cfg(test)]
mod tests;
