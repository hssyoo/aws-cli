//! Endpoint resolution for one call, as botocore's `EndpointRulesetResolver`
//! does it (botocore/regions.py): the parameters, from the operation, its
//! input and the client's built-ins, then the service's rule set,
//! evaluated by the CRT's rules engine.
//!
//! The client sends no endpoint headers, so a resolution that returns any
//! is refused ([`Unsupported::EndpointHeaders`]): of the rule sets this
//! botocore ships, only s3control's returns them.
use crate::admission::Unsupported;
use crate::error::CallError;
use model::shapes::{Metadata, Model, Operation};
use model::source::{Kind, LoadError, Source, load_data_bytes, load_service_bytes};
use model::value::Value;
use std::sync::{Arc, OnceLock};

/// A rule parameter's value, of a kind a rule set declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamValue {
    String(String),
    Boolean(bool),
    StringArray(Vec<String>),
}

/// The parameters of one resolution, in the order they were set; a name
/// left out is unset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EndpointParams(Vec<(String, ParamValue)>);

impl EndpointParams {
    /// Sets `name`, in place when it is set already.
    pub fn set(&mut self, name: &str, value: ParamValue) {
        match self.0.iter_mut().find(|(other, _)| other == name) {
            Some((_, old)) => *old = value,
            None => self.0.push((name.to_owned(), value)),
        }
    }
    pub fn get(&self, name: &str) -> Option<&ParamValue> {
        self.0
            .iter()
            .find(|(other, _)| other == name)
            .map(|(_, value)| value)
    }
    /// Unsets `name`.
    pub fn remove(&mut self, name: &str) {
        self.0.retain(|(other, _)| other != name);
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ParamValue)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }
}

/// A parameter the rule set declares (`parameters`), in its order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamDef {
    pub name: String,
    pub builtin: Option<String>,
    pub required: bool,
    pub has_default: bool,
}

/// Why a service's endpoint data could not be read.
#[derive(Debug)]
pub enum DataError {
    Load(LoadError),
    /// A document that is not the JSON its kind is.
    Invalid(&'static str),
}

impl std::fmt::Display for DataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(error) => error.fmt(f),
            Self::Invalid(what) => write!(f, "invalid {what}"),
        }
    }
}

impl std::error::Error for DataError {}

impl From<LoadError> for DataError {
    fn from(error: LoadError) -> Self {
        Self::Load(error)
    }
}

/// A service's rule set and the partitions it reads, with what the legacy
/// resolver says of a call with no region.
#[derive(Clone, Debug)]
pub struct EndpointData {
    rules: Vec<u8>,
    partitions: Vec<u8>,
    parameters: Vec<ParamDef>,
    /// `endpoints.json` and the service's key there, parsed only for a call
    /// that needs it ([`Legacy`]): parsing the whole document costs about
    /// 6 ms, and a call with a region and neither FIPS nor dual-stack needs
    /// none of it.
    legacy: Option<Arc<LegacyDocument>>,
}

/// `endpoints.json`'s bytes and, once parsed, what the client reads of it.
#[derive(Debug)]
struct LegacyDocument {
    bytes: Vec<u8>,
    prefix: String,
    parsed: OnceLock<Option<Legacy>>,
}

/// What the legacy resolver says for a client: the partition endpoint the
/// first partition gives the service, which a client with no region
/// resolves to, and the partitions, for the variant check.
#[derive(Debug)]
struct Legacy {
    partition_endpoint: Option<String>,
    partitions: Option<legacy::Legacy>,
}

impl LegacyDocument {
    /// The parsed document; `None` for one that is not the JSON its kind is.
    fn get(&self) -> Option<&Legacy> {
        self.parsed
            .get_or_init(|| {
                let document = document(&self.bytes, "endpoints.json").ok()?;
                Some(Legacy {
                    partition_endpoint: partition_endpoint(&document, &self.prefix).ok()?,
                    partitions: legacy::Legacy::parse(&document, &self.prefix),
                })
            })
            .as_ref()
    }
}

impl EndpointData {
    /// The latest `endpoint-rule-set-1` of `service`, `partitions` and
    /// `endpoints` from `source`, as botocore's client creation loads them.
    ///
    /// # Errors
    /// A file that is missing or is not the JSON its kind is.
    pub fn load(
        source: &impl Source,
        service: &str,
        metadata: &Metadata,
    ) -> Result<Self, DataError> {
        let (_, rules) = load_service_bytes(source, service, Kind::EndpointRuleSet)?;
        let partitions = load_data_bytes(source, "partitions")?;
        let endpoints = load_data_bytes(source, "endpoints")?;
        let prefix = metadata.endpoint_prefix.as_deref().unwrap_or(service);
        Self::from_documents(rules, partitions, Some((&endpoints, prefix)))
    }
    /// From the documents themselves: the rule set, the partitions, and
    /// `endpoints.json` with the service's endpoint prefix, its key there.
    ///
    /// # Errors
    /// A rule set without its parameters, or an `endpoints.json` that is
    /// not an object.
    pub fn from_documents(
        rules: Vec<u8>,
        partitions: Vec<u8>,
        endpoints: Option<(&[u8], &str)>,
    ) -> Result<Self, DataError> {
        let parameters = parameter_defs(&rules)?;
        let legacy = endpoints.map(|(bytes, prefix)| {
            Arc::new(LegacyDocument {
                bytes: bytes.to_vec(),
                prefix: prefix.to_owned(),
                parsed: OnceLock::new(),
            })
        });
        Ok(Self {
            rules,
            partitions,
            parameters,
            legacy,
        })
    }
    pub fn parameters(&self) -> &[ParamDef] {
        &self.parameters
    }
}

/// A JSON document as a [`Value`].
fn document(json: &[u8], what: &'static str) -> Result<Value, DataError> {
    Value::from_json(json).map_err(|_| DataError::Invalid(what))
}

fn map_field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Map(map) => map.get(name),
        _ => None,
    }
}

fn parameter_defs(rules: &[u8]) -> Result<Vec<ParamDef>, DataError> {
    let rules = document(rules, "endpoint rule set")?;
    let Some(Value::Map(parameters)) = map_field(&rules, "parameters") else {
        return Err(DataError::Invalid("endpoint rule set parameters"));
    };
    let flag = |def: &Value, key| matches!(map_field(def, key), Some(Value::Bool(true)));
    Ok(parameters
        .iter()
        .map(|(name, def)| ParamDef {
            name: name.to_owned(),
            builtin: match map_field(def, "builtIn") {
                Some(Value::String(builtin)) => Some(builtin.clone()),
                _ => None,
            },
            required: flag(def, "required"),
            has_default: map_field(def, "default").is_some_and(|value| *value != Value::Null),
        })
        .collect())
}

/// `EndpointResolver._endpoint_for_partition` with no region: the first
/// partition decides, and the service resolves only to its
/// `partitionEndpoint` there.
fn partition_endpoint(endpoints: &Value, prefix: &str) -> Result<Option<String>, DataError> {
    let Some(Value::List(partitions)) = map_field(endpoints, "partitions") else {
        return Err(DataError::Invalid("endpoints.json partitions"));
    };
    let service = partitions
        .first()
        .and_then(|partition| map_field(partition, "services"))
        .and_then(|services| map_field(services, prefix));
    Ok(
        match service.and_then(|service| map_field(service, "partitionEndpoint")) {
            Some(Value::String(name)) => Some(name.clone()),
            _ => None,
        },
    )
}

/// `ClientCreator._normalize_fips_region`: the region without `fips-` and
/// `-fips`, and whether it had either, which turns FIPS on.
pub fn normalize_fips_region(region: &str) -> (String, bool) {
    let normalized = region.replace("fips-", "").replace("-fips", "");
    let pseudo = normalized != region;
    (normalized, pseudo)
}

/// What a client's endpoint settings are given.
#[derive(Clone, Copy, Debug, Default)]
pub struct EndpointInputs<'a> {
    /// The session's region, `None` for none.
    pub region: Option<&'a str>,
    /// `use_fips_endpoint`, `use_dualstack_endpoint`, when configured.
    pub use_fips: Option<bool>,
    pub use_dual_stack: Option<bool>,
    /// `--endpoint-url` or a configured endpoint.
    pub endpoint_url: Option<&'a str>,
    /// Whether the client signs (not `--no-sign-request`).
    pub signing: bool,
}

/// A client's endpoint settings, as botocore's client creation computes
/// them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointConfig {
    /// The region as given, which `validate_region_name` reads.
    given_region: Option<String>,
    /// FIPS and dual-stack as the legacy resolver is asked for them,
    /// endpoint URL or not.
    variant_fips: bool,
    variant_dual_stack: bool,
    region: Option<String>,
    use_fips: bool,
    use_dual_stack: bool,
    endpoint_url: Option<String>,
    signing: bool,
}

impl EndpointConfig {
    /// The region normalized (`_normalize_fips_region`, which forces FIPS
    /// on), and FIPS and dual-stack off with an endpoint given, as
    /// `compute_endpoint_resolver_builtin_defaults` sets their built-ins.
    pub fn new(inputs: &EndpointInputs<'_>) -> Self {
        let (region, pseudo) = match inputs.region.map(normalize_fips_region) {
            Some((region, pseudo)) => (Some(region), pseudo),
            None => (None, false),
        };
        let custom = inputs.endpoint_url.is_some();
        Self {
            given_region: inputs.region.map(str::to_owned),
            variant_fips: pseudo || inputs.use_fips.unwrap_or(false),
            variant_dual_stack: inputs.use_dual_stack.unwrap_or(false),
            region,
            use_fips: !custom && (pseudo || inputs.use_fips.unwrap_or(false)),
            use_dual_stack: !custom && inputs.use_dual_stack.unwrap_or(false),
            endpoint_url: inputs.endpoint_url.map(str::to_owned),
            signing: inputs.signing,
        }
    }
    pub fn region(&self) -> Option<&str> {
        self.region.as_deref()
    }
    pub const fn use_fips(&self) -> bool {
        self.use_fips
    }
    pub const fn use_dual_stack(&self) -> bool {
        self.use_dual_stack
    }
    pub fn endpoint_url(&self) -> Option<&str> {
        self.endpoint_url.as_deref()
    }
    /// `AWS::Auth::AccountIdEndpointMode`: `disabled` for a client that
    /// does not sign (`_compute_account_id_endpoint_mode_config`), else the
    /// configured mode's default, `preferred`. A configured mode never
    /// reaches the native client.
    pub const fn account_id_endpoint_mode(&self) -> &'static str {
        if self.signing {
            "preferred"
        } else {
            "disabled"
        }
    }
    /// What client creation refuses before anything resolves: a region
    /// that is no host label (`validate_region_name`), and FIPS or
    /// dual-stack without the variant in `endpoints.json`
    /// (`EndpointVariantError`, [`legacy`]).
    ///
    /// # Errors
    /// Their text; [`Unsupported::RegionPattern`] for a partition pattern
    /// the variant check cannot read.
    pub fn check(&self, data: &EndpointData) -> Result<(), CallError> {
        if let Some(region) = self.given_region.as_deref()
            && !cli_core::region::is_valid_name(region)
        {
            return Err(CallError::Endpoint(format!(
                "Provided region_name '{region}' doesn't match a supported format."
            )));
        }
        if !(self.variant_fips || self.variant_dual_stack) {
            return Ok(());
        }
        let Some(document) = &data.legacy else {
            return Ok(());
        };
        let legacy = document
            .get()
            .ok_or(CallError::Unsupported(Unsupported::EndpointData))?;
        let region = self.region.as_ref().or(legacy.partition_endpoint.as_ref());
        let (Some(region), Some(partitions)) = (region, &legacy.partitions) else {
            return Ok(());
        };
        match partitions.check(region, self.variant_dual_stack, self.variant_fips) {
            Ok(()) => Ok(()),
            Err(legacy::Refusal::Variant(text)) => Err(CallError::Endpoint(text)),
            Err(legacy::Refusal::Pattern) => {
                Err(CallError::Unsupported(Unsupported::RegionPattern))
            }
        }
    }
    /// The region the client resolves with (`ClientEndpointBridge.resolve`):
    /// the configured one, else the legacy resolver's for no region, the
    /// service's partition endpoint, which botocore's client takes as its
    /// region (`_pick_region_values`); with an endpoint given that leaves
    /// none.
    ///
    /// # Errors
    /// `NoRegionError` for no region and no partition endpoint, which the
    /// legacy resolver raises as the client is created, endpoint or not.
    pub fn client_region(&self, data: &EndpointData) -> Result<Option<String>, CallError> {
        if let Some(region) = &self.region {
            return Ok(Some(region.clone()));
        }
        let partition_endpoint = match &data.legacy {
            Some(document) => {
                let legacy = document
                    .get()
                    .ok_or(CallError::Unsupported(Unsupported::EndpointData))?;
                legacy.partition_endpoint.as_ref()
            }
            None => None,
        };
        match partition_endpoint {
            None => Err(CallError::NoRegion),
            Some(_) if self.endpoint_url.is_some() => Ok(None),
            Some(name) => Ok(Some(name.clone())),
        }
    }
    /// The built-ins core supplies for `data`'s service. `AWS::Region` is
    /// the [`Self::client_region`], which is what `args.py`'s region rule
    /// passes for every service once a region resolves: the legacy
    /// endpoint's name is the configured region whenever one is set.
    ///
    /// # Errors
    /// See [`Self::client_region`].
    pub fn builtins(&self, data: &EndpointData) -> Result<Builtins, CallError> {
        let mut builtins = Builtins::default();
        if let Some(region) = self.client_region(data)? {
            builtins.set("AWS::Region", ParamValue::String(region));
        }
        builtins.set("AWS::UseFIPS", ParamValue::Boolean(self.use_fips));
        builtins.set(
            "AWS::UseDualStack",
            ParamValue::Boolean(self.use_dual_stack),
        );
        // Constants botocore gives every client
        // (`compute_endpoint_resolver_builtin_defaults`).
        builtins.set("AWS::STS::UseGlobalEndpoint", ParamValue::Boolean(false));
        builtins.set("AWS::S3::UseGlobalEndpoint", ParamValue::Boolean(false));
        if let Some(endpoint) = &self.endpoint_url {
            builtins.set("SDK::Endpoint", ParamValue::String(endpoint.clone()));
        }
        builtins.set(
            "AWS::Auth::AccountIdEndpointMode",
            ParamValue::String(self.account_id_endpoint_mode().to_owned()),
        );
        Ok(builtins)
    }
}

/// Built-in values by `builtIn` name (`EndpointResolverBuiltins`): core's,
/// then what customizations set at the `endpoint_builtins` stage, as
/// botocore's `before-endpoint-resolution` handlers change the built-ins
/// dict. `AWS::Auth::AccountId` is never here: it is deferred to the
/// credentials, as botocore's `get_deferred_property` defers it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Builtins(EndpointParams);

impl Builtins {
    /// Sets the built-in `name`.
    pub fn set(&mut self, name: &str, value: ParamValue) {
        self.0.set(name, value);
    }
    pub fn get(&self, name: &str) -> Option<&ParamValue> {
        self.0.get(name)
    }
    /// Unsets the built-in `name`, which then leaves its parameter unset.
    pub fn remove(&mut self, name: &str) {
        self.0.remove(name);
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ParamValue)> {
        self.0.iter()
    }
}

/// The built-ins core supplies, `AWS::Auth::AccountId` among them by
/// deferral. Any other is a customization's to supply.
const CORE_BUILTINS: [&str; 8] = [
    "AWS::Region",
    "AWS::UseFIPS",
    "AWS::UseDualStack",
    "AWS::STS::UseGlobalEndpoint",
    "AWS::S3::UseGlobalEndpoint",
    "SDK::Endpoint",
    "AWS::Auth::AccountId",
    "AWS::Auth::AccountIdEndpointMode",
];

/// The first built-in the rule set requires without a default that neither
/// core nor a customization in scope for `service` supplies (a
/// customization names the built-ins it sets,
/// [`crate::hooks::Customization::builtins`]).
pub fn missing_builtin(
    data: &EndpointData,
    service: &str,
    customizations: &[&'static crate::hooks::Customization],
) -> Option<String> {
    let supplied = |builtin: &str| {
        CORE_BUILTINS.contains(&builtin)
            || customizations.iter().any(|customization| {
                customization.scope.matches_service(service)
                    && customization.builtins.contains(&builtin)
            })
    };
    data.parameters
        .iter()
        .filter(|def| def.required && !def.has_default)
        .find_map(|def| def.builtin.as_deref().filter(|builtin| !supplied(builtin)))
        .map(str::to_owned)
}

/// `EndpointRulesetResolver._get_provider_params`: for each parameter the
/// rule set declares, in its order, the operation's static context value,
/// else the input member's that names it (`contextParam`), else the
/// built-in's, so a customization's built-in never overrides a context
/// parameter. `account_id` is the deferred `AWS::Auth::AccountId`, `None`
/// until the credentials are known.
pub fn provider_params(
    data: &EndpointData,
    model: &Model,
    operation: &Operation,
    input: &Value,
    builtins: &Builtins,
    account_id: Option<&str>,
) -> EndpointParams {
    let mut params = EndpointParams::default();
    for def in &data.parameters {
        let value = static_param(operation, &def.name)
            .or_else(|| context_param(model, operation, input, &def.name))
            .or_else(|| match def.builtin.as_deref()? {
                "AWS::Auth::AccountId" => account_id.map(|id| ParamValue::String(id.to_owned())),
                builtin => builtins.get(builtin).cloned(),
            });
        if let Some(value) = value {
            params.set(&def.name, value);
        }
    }
    params
}

fn static_param(operation: &Operation, name: &str) -> Option<ParamValue> {
    let (_, value) = operation
        .static_context_params
        .iter()
        .find(|(param, _)| &**param == name)?;
    param_value(value)
}

/// `OperationModel.context_parameters`: the input member whose
/// `contextParam` names `name`, and its value in `input`.
fn context_param(
    model: &Model,
    operation: &Operation,
    input: &Value,
    name: &str,
) -> Option<ParamValue> {
    let shape = operation.input?;
    let member = model
        .members(shape)
        .iter()
        .find(|member| model.shape(member.shape).traits.context_param.as_deref() == Some(name))?;
    let Value::Map(input) = input else {
        return None;
    };
    param_value(input.get(&member.name)?)
}

fn param_value(value: &Value) -> Option<ParamValue> {
    match value {
        Value::String(text) => Some(ParamValue::String(text.clone())),
        Value::Bool(flag) => Some(ParamValue::Boolean(*flag)),
        Value::List(items) => items
            .iter()
            .map(|item| match item {
                Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(ParamValue::StringArray),
        _ => None,
    }
}

/// Whether the rule set reads the deferred `AWS::Auth::AccountId`, so a
/// call's endpoint can change once the credentials carry an account id.
pub fn reads_account_id(data: &EndpointData) -> bool {
    data.parameters
        .iter()
        .any(|def| def.builtin.as_deref() == Some("AWS::Auth::AccountId"))
}

/// An `authSchemes` entry of a resolved endpoint's properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemeProps {
    pub name: String,
    pub signing_name: Option<String>,
    pub signing_region: Option<String>,
    pub signing_region_set: Option<Vec<String>>,
    pub disable_double_encoding: Option<bool>,
}

/// A resolved endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedEndpoint {
    pub url: String,
    pub auth_schemes: Vec<SchemeProps>,
}

/// What botocore signs with from the endpoint (`auth_schemes_to_signing_ctx`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningContext {
    /// botocore's auth type for the scheme, its name without `sig`.
    pub auth_type: String,
    pub region: Option<String>,
    pub name: Option<String>,
    pub disable_double_encoding: Option<bool>,
}

/// botocore's `AUTH_TYPE_MAPS` keys: the auth types it can sign with.
const BOTOCORE_AUTH_TYPES: [&str; 15] = [
    "v2",
    "v4",
    "v4a",
    "v4-query",
    "v3",
    "v3https",
    "s3v4",
    "s3v4-query",
    "s3v4-presign-post",
    "s3v4a",
    "s3v4a-query",
    "v4-s3express",
    "v4-s3express-query",
    "v4-s3express-presign-post",
    "bearer",
];

impl ResolvedEndpoint {
    /// `auth_schemes_to_signing_ctx` with no scheme requested: the first
    /// scheme botocore signs with, and its signing region (a region set is
    /// joined with commas) and name; `None` for an endpoint without
    /// schemes.
    ///
    /// # Errors
    /// `UnknownSignatureVersionError`'s text when botocore signs with none.
    pub fn signing_context(&self) -> Result<Option<SigningContext>, String> {
        if self.auth_schemes.is_empty() {
            return Ok(None);
        }
        let strip = |name: &str| name.strip_prefix("sig").unwrap_or(name).to_owned();
        let Some(scheme) = self
            .auth_schemes
            .iter()
            .find(|scheme| BOTOCORE_AUTH_TYPES.contains(&strip(&scheme.name).as_str()))
        else {
            let names: Vec<&str> = self.auth_schemes.iter().map(|s| s.name.as_str()).collect();
            return Err(format!("Unknown Signature Version: {}.", names.join(", ")));
        };
        let region = scheme.signing_region.clone().or_else(|| {
            let set = scheme.signing_region_set.as_ref()?;
            (!set.is_empty()).then(|| set.join(","))
        });
        Ok(Some(SigningContext {
            auth_type: strip(&scheme.name),
            region,
            name: scheme.signing_name.clone(),
            disable_double_encoding: scheme.disable_double_encoding,
        }))
    }
}

/// Evaluates `data`'s rule set for `params` with the CRT's engine
/// (`EndpointProvider.resolve_endpoint`).
///
/// # Errors
/// `EndpointResolutionError`, with the rule's message, for an error rule;
/// botocore's `InvalidHostLabelError` for one that names an invalid region
/// in an ARN (`ruleset_error_to_botocore_exception`).
pub fn resolve(
    data: &EndpointData,
    params: &EndpointParams,
) -> Result<ResolvedEndpoint, CallError> {
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
    match crt::endpoints::resolve_with_headers(documents, &list) {
        // The client sends no endpoint headers (only s3control's rule set
        // returns any today), so a resolution with them is Python's.
        Ok((Resolved::Endpoint { .. }, headers)) if headers > 0 => {
            Err(CallError::Unsupported(Unsupported::EndpointHeaders))
        }
        Ok((Resolved::Endpoint { url, properties }, _)) => Ok(ResolvedEndpoint {
            url: String::from_utf8_lossy(&url).into_owned(),
            auth_schemes: auth_schemes(&properties),
        }),
        Ok((Resolved::RuleError(message), _)) => Err(CallError::Endpoint(rule_error_text(
            &String::from_utf8_lossy(&message),
        ))),
        Err(error) => Err(CallError::Endpoint(error.to_string())),
    }
}

/// `ruleset_error_to_botocore_exception` for every service: an invalid
/// region in an ARN is `InvalidHostLabelError`; any other message is
/// `EndpointResolutionError`'s, as given.
fn rule_error_text(message: &str) -> String {
    match message.strip_prefix("Invalid region in ARN: ") {
        Some(rest) => {
            let label = rest.split('`').nth(1).unwrap_or(message);
            format!("Invalid host label to be bound to the hostname of the endpoint: \"{label}\".")
        }
        None => message.to_owned(),
    }
}

/// The `authSchemes` of a resolved endpoint's properties.
fn auth_schemes(properties: &[u8]) -> Vec<SchemeProps> {
    let Ok(properties) = Value::from_json(properties) else {
        return Vec::new();
    };
    let Some(Value::List(schemes)) = map_field(&properties, "authSchemes") else {
        return Vec::new();
    };
    let text = |scheme: &Value, key| match map_field(scheme, key) {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    };
    schemes
        .iter()
        .map(|scheme| SchemeProps {
            name: text(scheme, "name").unwrap_or_default(),
            signing_name: text(scheme, "signingName"),
            signing_region: text(scheme, "signingRegion"),
            signing_region_set: match map_field(scheme, "signingRegionSet") {
                Some(Value::List(regions)) => Some(
                    regions
                        .iter()
                        .filter_map(|region| match region {
                            Value::String(region) => Some(region.clone()),
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            },
            disable_double_encoding: match map_field(scheme, "disableDoubleEncoding") {
                Some(Value::Bool(flag)) => Some(*flag),
                _ => None,
            },
        })
        .collect()
}

mod legacy;

#[cfg(test)]
mod tests;
