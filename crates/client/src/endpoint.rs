//! Endpoint resolution for one call, as botocore's `EndpointRulesetResolver`
//! does it (botocore/regions.py): the parameters, from the operation, its
//! input and the client's built-ins, then the service's rule set,
//! evaluated by the native rules engine (`endpoint-rules`), which a client
//! decodes once and its clones share.
//!
//! The client sends no endpoint headers, so a resolution that returns any
//! is refused ([`Unsupported::EndpointHeaders`]): of the rule sets this
//! botocore ships, only s3control's returns them.
use crate::admission::Unsupported;
use crate::error::CallError;
use endpoint_rules::{DecodeError, EvalValue, Library, Outcome, Params, Partitions, Program};
use model::schema::{OperationSchema, ServiceSchema};
use model::shapes::Model;
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

/// A decoded document, decoded on first use and shared by every clone.
type Decoded<T> = Arc<OnceLock<Result<Arc<T>, DecodeError>>>;

/// A service's rule set and the partitions it reads, with what the legacy
/// resolver says of a call with no region. A clone (one per client, and so
/// per page) shares the documents and their decode, so each is decoded at
/// most once.
#[derive(Clone, Debug)]
pub struct EndpointData {
    rules: Arc<[u8]>,
    partitions: Arc<[u8]>,
    program: Decoded<Program>,
    partition_table: Decoded<Partitions>,
    parameters: Arc<[ParamDef]>,
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
        schema: ServiceSchema<'_>,
    ) -> Result<Self, DataError> {
        let (_, rules) = load_service_bytes(source, service, Kind::EndpointRuleSet)?;
        let partitions = load_data_bytes(source, "partitions")?;
        let endpoints = load_data_bytes(source, "endpoints")?;
        let prefix = schema.endpoint_prefix().unwrap_or(service);
        Self::from_documents(rules, partitions, Some((&endpoints, prefix)))
    }
    /// From the documents themselves: the rule set, the partitions, and
    /// `endpoints.json` with the service's endpoint prefix, its key there.
    ///
    /// # Errors
    /// A rule set without its parameters, or an `endpoints.json` that is
    /// not an object.
    pub fn from_documents(
        rules: impl Into<Arc<[u8]>>,
        partitions: impl Into<Arc<[u8]>>,
        endpoints: Option<(&[u8], &str)>,
    ) -> Result<Self, DataError> {
        let rules = rules.into();
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
            partitions: partitions.into(),
            program: Arc::default(),
            partition_table: Arc::default(),
            parameters: parameters.into(),
            legacy,
        })
    }
    pub fn parameters(&self) -> &[ParamDef] {
        &self.parameters
    }
    /// The decoded rule set and partitions, in the order the CRT engine
    /// read them, so a rule set that fails reports before its partitions.
    fn decoded(&self) -> Result<(&Program, &Partitions), &DecodeError> {
        fn get<T>(
            cell: &Decoded<T>,
            decode: impl FnOnce() -> Result<T, DecodeError>,
        ) -> Result<&T, &DecodeError> {
            cell.get_or_init(|| decode().map(Arc::new))
                .as_ref()
                .map(|decoded| &**decoded)
        }
        let program = get(&self.program, || Program::decode(&self.rules))?;
        let partitions = get(&self.partition_table, || {
            Partitions::decode(&self.partitions)
        })?;
        Ok((program, partitions))
    }
}

/// A JSON document as a [`Value`].
fn document(json: &[u8], what: &'static str) -> Result<Value, DataError> {
    Value::from_json(json).map_err(|_| DataError::Invalid(what))
}

fn parameter_defs(rules: &[u8]) -> Result<Vec<ParamDef>, DataError> {
    let rules = document(rules, "endpoint rule set")?;
    let Some(Value::Map(parameters)) = &rules.get("parameters") else {
        return Err(DataError::Invalid("endpoint rule set parameters"));
    };
    let flag = |def: &Value, key| matches!(def.get(key), Some(Value::Bool(true)));
    Ok(parameters
        .iter()
        .map(|(name, def)| ParamDef {
            // Bundled data, which `Value::from_json` reads into `Str` keys.
            name: name.text().into_owned(),
            builtin: match def.get("builtIn") {
                Some(Value::String(builtin)) => Some(builtin.clone()),
                _ => None,
            },
            required: flag(def, "required"),
            has_default: def
                .get("default")
                .is_some_and(|value| *value != Value::Null),
        })
        .collect())
}

/// `EndpointResolver._endpoint_for_partition` with no region: the first
/// partition decides, and the service resolves only to its
/// `partitionEndpoint` there.
fn partition_endpoint(endpoints: &Value, prefix: &str) -> Result<Option<String>, DataError> {
    let Some(Value::List(partitions)) = endpoints.get("partitions") else {
        return Err(DataError::Invalid("endpoints.json partitions"));
    };
    let service = partitions
        .first()
        .and_then(|partition| partition.get("services"))
        .and_then(|services| services.get(prefix));
    Ok(
        match service.and_then(|service| service.get("partitionEndpoint")) {
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
/// else the input member's that names it (`contextParam`), else the client
/// context's (`_resolve_param_as_client_context_param`), which in the CLI
/// is empty ([`missing_client_context`]), else the built-in's, so a
/// customization's built-in never overrides a context parameter.
/// `account_id` is the deferred `AWS::Auth::AccountId`, `None` until the
/// credentials are known.
pub fn provider_params(
    data: &EndpointData,
    operation: OperationSchema<'_>,
    input: &Value,
    builtins: &Builtins,
    account_id: Option<&str>,
) -> EndpointParams {
    let mut params = EndpointParams::default();
    for def in data.parameters.iter() {
        let value = static_param(operation, &def.name)
            .or_else(|| context_param(operation, input, &def.name))
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

/// The first parameter the rule set requires, without a default or a
/// built-in, that only the client context could give `operation`: one the
/// model declares in `clientContextParams` and the operation neither sets
/// (`staticContextParams`) nor binds to an input member (`contextParam`).
/// The AWS CLI's client context is empty: it builds its clients with no
/// `Config(client_context_params=...)`, and only an S3 client folds its `s3`
/// configuration section into it (args.py
/// `ClientArgsCreator.compute_client_args`), a protocol the client does not
/// implement. So nothing supplies such a parameter.
pub fn missing_client_context(
    data: &EndpointData,
    model: &Model,
    operation: OperationSchema<'_>,
) -> Option<String> {
    let bound = |name: &str| {
        static_param(operation, name).is_some() || context_member(operation, name).is_some()
    };
    let declared = model.schema().service().client_context_params();
    data.parameters
        .iter()
        .filter(|def| def.required && !def.has_default && def.builtin.is_none())
        .filter(|def| declared.iter().any(|param| *param.name == *def.name))
        .find(|def| !bound(&def.name))
        .map(|def| def.name.clone())
}

fn static_param(operation: OperationSchema<'_>, name: &str) -> Option<ParamValue> {
    let (_, value) = operation
        .static_context_params()
        .find(|(param, _)| *param == name)?;
    param_value(value)
}

/// The first input member whose `contextParam` names `name`.
fn context_member<'s>(operation: OperationSchema<'s>, name: &str) -> Option<&'s str> {
    operation
        .input()?
        .members()
        .find(|member| member.context_param() == Some(name))?
        .member_name()
}

/// `OperationModel.context_parameters`: the input member whose
/// `contextParam` names `name`, and its value in `input`.
fn context_param(operation: OperationSchema<'_>, input: &Value, name: &str) -> Option<ParamValue> {
    let member = context_member(operation, name)?;
    let Value::Map(input) = input else {
        return None;
    };
    param_value(input.get(member)?)
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
            let names: Vec<String> = self.auth_schemes.iter().map(|s| strip(&s.name)).collect();
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

/// Evaluates `data`'s rule set for `params`
/// (`EndpointProvider.resolve_endpoint`). A parameter the rule set does not
/// declare is dropped.
///
/// # Errors
/// `EndpointResolutionError`, with the rule's message, for an error rule;
/// botocore's `InvalidHostLabelError` for one that names an invalid region
/// in an ARN (`ruleset_error_to_botocore_exception`); the engine's text for
/// a document it cannot decode or a rule set it cannot evaluate.
pub fn resolve(
    data: &EndpointData,
    params: &EndpointParams,
) -> Result<ResolvedEndpoint, CallError> {
    match evaluate(data, params.iter()).map_err(CallError::Endpoint)? {
        // The client sends no endpoint headers (only s3control's rule set
        // returns any today), so a resolution with them is Python's.
        Outcome::Endpoint(endpoint) if !endpoint.headers.is_empty() => {
            Err(CallError::Unsupported(Unsupported::EndpointHeaders))
        }
        Outcome::Endpoint(endpoint) => Ok(ResolvedEndpoint {
            auth_schemes: schemes_of(&endpoint.properties),
            url: endpoint.url,
        }),
        Outcome::Error(message) => Err(CallError::Endpoint(rule_error_text(&message))),
    }
}

/// `EndpointProvider.resolve_endpoint(**params)`: `data`'s rule set
/// evaluated for `params`, each bound by name; a name the rule set does
/// not declare is dropped, and a name given twice takes its last value.
///
/// # Errors
/// The engine's text for a document it cannot decode or a rule set it
/// cannot evaluate.
pub(crate) fn evaluate<'a>(
    data: &EndpointData,
    params: impl IntoIterator<Item = (&'a str, &'a ParamValue)>,
) -> Result<Outcome, String> {
    let (program, partitions) = data.decoded().map_err(ToString::to_string)?;
    let mut args = Params::new(program);
    for (name, value) in params {
        if let Some(def) = program.param(name) {
            args.set(def.slot(), eval_value(value));
        }
    }
    program
        .evaluate(&args, &Library { partitions })
        .map_err(|error| error.to_string())
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

/// The `authSchemes` of a resolved endpoint's properties, as JSON text.
#[cfg(test)]
fn auth_schemes(properties: &[u8]) -> Vec<SchemeProps> {
    serde_json::from_slice(properties).map_or_else(|_| Vec::new(), |value| schemes_of(&value))
}

/// The `authSchemes` of a resolved endpoint's properties: each entry's
/// strings and flag, a member of another type left unset.
fn schemes_of(properties: &serde_json::Value) -> Vec<SchemeProps> {
    let Some(schemes) = properties.get("authSchemes").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let text = |scheme: &serde_json::Value, key| {
        scheme.get(key).and_then(|v| v.as_str()).map(str::to_owned)
    };
    schemes
        .iter()
        .map(|scheme| SchemeProps {
            name: text(scheme, "name").unwrap_or_default(),
            signing_name: text(scheme, "signingName"),
            signing_region: text(scheme, "signingRegion"),
            signing_region_set: scheme
                .get("signingRegionSet")
                .and_then(|v| v.as_array())
                .map(|regions| {
                    regions
                        .iter()
                        .filter_map(|region| region.as_str().map(str::to_owned))
                        .collect()
                }),
            disable_double_encoding: scheme
                .get("disableDoubleEncoding")
                .and_then(|v| v.as_bool()),
        })
        .collect()
}

mod legacy;

#[cfg(test)]
mod tests;
