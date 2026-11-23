//! The settings every botocore client of an invocation resolves, whatever
//! its service: trust, signing, retries, timeouts, the socket's keepalive,
//! the endpoint variants, and the User-Agent's inputs. The S3 commands'
//! record ([`ClientConfig`]) holds them beside its own; a model operation's
//! run holds them alone ([`resolve_settings`]). Each setting has one reader
//! here, which both resolutions call.
use super::*;
use crate::endpoint_config::{self, Variant};

/// What any client the invocation builds needs from its configuration.
/// A setting a customization may default is an `Option` that stays `None`
/// when nothing configures it, so the default applies first and the user's
/// value last: botocore passes `max_attempts` on only when the user set it
/// (client.py `ClientCreator._register_v2_standard_retries`), and only then
/// does `retries/standard.py` `register_retry_handler` skip its per-service
/// `_SERVICE_MAX_ATTEMPTS`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientSettings {
    /// `md/command#<lineage>`: the command's names joined by dots
    /// (`add_command_lineage_to_user_agent_extra`), such as `s3.cp`.
    pub command: String,
    pub trust: Resolved<Trust>,
    pub signing: Resolved<bool>,
    /// `retry_mode`, when configured; botocore's default is `standard`.
    pub retry_mode: Option<Resolved<RetryMode>>,
    /// `max_attempts`, when configured.
    pub max_attempts: Option<Resolved<u64>>,
    pub request_checksum: Resolved<ChecksumMode>,
    pub response_checksum: Resolved<ChecksumMode>,
    pub keep_alive: Resolved<bool>,
    pub connect_timeout_ms: Resolved<u32>,
    /// `--cli-read-timeout` in milliseconds, which [`timeout_milliseconds`]
    /// reads as it reads the connect timeout.
    pub read_timeout_ms: Resolved<u32>,
    /// `use_fips_endpoint`, when configured. The endpoint resolution that
    /// consumes it clears it for an explicit endpoint
    /// (`endpoint_config::resolve`).
    pub use_fips_endpoint: Option<Resolved<bool>>,
    /// `use_dualstack_endpoint`, when configured, cleared in the same way.
    pub use_dualstack_endpoint: Option<Resolved<bool>>,
    /// The values the user agent reports as given, kept as bytes: one from
    /// the environment need not be Unicode.
    pub app_id: Option<Resolved<Vec<u8>>>,
    pub execution_env: Option<Resolved<Vec<u8>>>,
    /// Whether any setting selects an endpoint URL for the service.
    pub endpoint_override: bool,
    pub agentic_callers: user_agent::Features,
    /// The provider the credential chain answers from.
    pub credential_source: CredentialSource,
}
impl ClientSettings {
    /// The settings before any configuration is read, for `command`.
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            trust: Resolved::default(),
            signing: Resolved::new(true, Source::Default),
            retry_mode: None,
            max_attempts: None,
            request_checksum: Resolved::default(),
            response_checksum: Resolved::default(),
            keep_alive: Resolved::default(),
            connect_timeout_ms: Resolved::new(TIMEOUT_MS_DEFAULT, Source::Default),
            read_timeout_ms: Resolved::new(TIMEOUT_MS_DEFAULT, Source::Default),
            use_fips_endpoint: None,
            use_dualstack_endpoint: None,
            app_id: None,
            execution_env: None,
            endpoint_override: false,
            agentic_callers: user_agent::Features::default(),
            credential_source: CredentialSource::DefaultChain,
        }
    }
    /// The feature ids every client's requests register that the settings
    /// decide: the agentic callers, a configured endpoint
    /// (`ClientArgsCreator.compute_client_args`), and the two checksum modes
    /// (`_register_checksum_config_feature_ids`), which every client
    /// registers as it is built.
    pub fn user_agent_features(&self) -> user_agent::Features {
        use user_agent::Feature;
        let mut features = self.agentic_callers;
        if self.endpoint_override {
            features.insert(Feature::EndpointOverride);
        }
        features.insert(match self.request_checksum.value {
            ChecksumMode::WhenSupported => Feature::FlexibleChecksumsReqWhenSupported,
            ChecksumMode::WhenRequired => Feature::FlexibleChecksumsReqWhenRequired,
        });
        features.insert(match self.response_checksum.value {
            ChecksumMode::WhenSupported => Feature::FlexibleChecksumsResWhenSupported,
            ChecksumMode::WhenRequired => Feature::FlexibleChecksumsResWhenRequired,
        });
        features
    }
    /// The feature ids the credential provider that answered registers
    /// (credentials.py `EnvProvider.load`, `ProcessProvider.load`,
    /// `SharedCredentialProvider`, `ContainerProvider`): none for a client
    /// that does not sign, or for a source the chain does not name.
    pub fn credential_features(&self) -> &'static [user_agent::Feature] {
        use user_agent::Feature;
        if !self.signing.value {
            return &[];
        }
        match self.credential_source {
            CredentialSource::Environment => &[Feature::CredentialsEnvVars],
            CredentialSource::SharedProfile => &[Feature::CredentialsProfile],
            CredentialSource::Process => &[
                Feature::CredentialsProfileProcess,
                Feature::CredentialsProcess,
            ],
            CredentialSource::Container => &[Feature::CredentialsHttp],
            CredentialSource::DefaultChain => &[],
        }
    }
    /// The retry mode the client runs: the configured one, or botocore's
    /// `standard` (`ClientArgsCreator._compute_retry_mode`).
    pub fn retry_mode(&self) -> Resolved<RetryMode> {
        self.retry_mode
            .clone()
            .unwrap_or(Resolved::new(RetryMode::Standard, Source::Default))
    }
}

/// What the settings are resolved from, for a client of one service.
#[derive(Debug)]
pub struct SettingsInputs<'a, E: EnvMap> {
    pub env: &'a E,
    pub files: &'a ParsedFiles,
    pub globals: &'a Globals,
    pub directory: &'a Path,
    /// Whether the payload embeds botocore's bundle ([`CACERT_ENTRY_NAME`]).
    pub embedded_trust: bool,
    /// The service as the configured-endpoint lookup names it: the model's
    /// `serviceId`, hyphenated and lowercased (`ConfiguredEndpointProvider`).
    pub service: &'a str,
    /// The User-Agent's command label ([`ClientSettings::command`]).
    pub command: &'a str,
}
impl<'a, E: EnvMap> SettingsInputs<'a, E> {
    pub(super) fn profile(&self) -> &'a OsStr {
        aws_profile::select_or_default(self.env, self.globals.profile.as_deref())
    }
    pub(super) fn property(&self, key: &str) -> Option<&'a str> {
        shared_config::profile_property(self.files, self.profile(), key)
    }
    pub(super) fn nested(&self, key: &str) -> Option<&'a str> {
        shared_config::profile_nested_property(self.files, self.profile(), "s3", key)
    }
    pub(super) fn first_os(
        &self,
        field: Field,
    ) -> Result<Option<Resolved<&'a OsStr>>, PythonReason> {
        for &source in field.settings() {
            let value = match source {
                Source::Env(name) => self.env.var_os(name),
                Source::Profile(key) => {
                    if shared_config::profile_property_is_block(self.files, self.profile(), key) {
                        return Err(field.reason());
                    }
                    self.property(key).map(OsStr::new)
                }
                Source::S3(key) => self.nested(key).map(OsStr::new),
                Source::Default | Source::Persona | Source::Flag(_) => None,
            };
            if let Some(value) = value {
                return Ok(Some(Resolved::new(value, source)));
            }
        }
        Ok(None)
    }
    pub(super) fn first(&self, field: Field) -> Result<Option<Resolved<&'a str>>, PythonReason> {
        let Some(v) = self.first_os(field)? else {
            return Ok(None);
        };
        let value = v.value.to_str().ok_or(field.reason())?;
        Ok(Some(Resolved::new(value, v.source)))
    }
}

/// The settings of a client of a model operation, in the order botocore
/// raises their errors as it builds the client: the top-level arguments,
/// then the client's arguments, then the `Config` they build, which checks
/// the `s3` block's addressing style for every client, then the
/// User-Agent. The bundle's contents come
/// last, as they do for `s3 ls`, whose client loads them the same way.
/// `adaptive` retries are refused after every check botocore makes: the
/// native client runs the standard mode only.
pub fn resolve_settings<E: EnvMap>(
    inputs: &SettingsInputs<'_, E>,
) -> Result<ClientSettings, PythonReason> {
    let mut settings = start(inputs);
    resolve_globals(&mut settings, inputs.globals)?;
    resolve_client_arguments(inputs, &mut settings)?;
    s3_addressing_style(inputs)?;
    user_agent::resolve(inputs, &mut settings)?;
    if settings.retry_mode().value == RetryMode::Adaptive {
        return Err(PythonReason::RetrySetting);
    }
    if let Trust::File(path) = &settings.trust.value
        && !holds_only_certificates(inputs.directory, path)
    {
        return Err(trust_reason(settings.trust.source));
    }
    Ok(settings)
}
/// The settings before any is read, with the provider the credential chain
/// would answer from.
pub(super) fn start<E: EnvMap>(inputs: &SettingsInputs<'_, E>) -> ClientSettings {
    let mut settings = ClientSettings::new(inputs.command);
    settings.credential_source =
        CredentialSource::select(inputs.env, inputs.files, inputs.globals.profile.as_deref());
    settings
}
/// The reads `ClientArgsCreator.compute_client_args` and the endpoint
/// bridge make for every client, in [`Field::ALL`] order: the retry
/// settings, the checksum settings, the keepalive switch, the CA bundle,
/// the proxy environment, then the endpoint variants, which never fail.
pub(super) fn resolve_client_arguments<E: EnvMap>(
    inputs: &SettingsInputs<'_, E>,
    settings: &mut ClientSettings,
) -> Result<(), PythonReason> {
    resolve_max_attempts(inputs, settings)?;
    resolve_checksum(
        inputs,
        Field::RequestChecksumCalculation,
        &mut settings.request_checksum,
    )?;
    resolve_checksum(
        inputs,
        Field::ResponseChecksumValidation,
        &mut settings.response_checksum,
    )?;
    resolve_retry_mode(inputs, settings)?;
    resolve_keep_alive(inputs, settings)?;
    resolve_trust(inputs, settings)?;
    resolve_proxy(inputs)?;
    let profile = inputs.profile();
    let variant = |variant: Variant| {
        endpoint_config::variant(inputs.env, inputs.files, profile, variant).map(|switch| {
            let source = if switch.from_variable {
                Source::Env(variant.variable())
            } else {
                Source::Profile(variant.key())
            };
            Resolved::new(switch.on, source)
        })
    };
    settings.use_fips_endpoint = variant(Variant::Fips);
    settings.use_dualstack_endpoint = variant(Variant::DualStack);
    Ok(())
}
/// The `s3` block's `addressing_style`, which every client's `Config`
/// validates (`Config._validate_s3_configuration`, through
/// `ClientArgsCreator.get_client_args`): `auto`, `virtual` or `path`, and
/// any other value is InvalidS3AddressingStyleError. A profile `s3` key
/// that is no block is no configuration (`SectionConfigProvider`).
pub(super) fn s3_addressing_style<E: EnvMap>(
    inputs: &SettingsInputs<'_, E>,
) -> Result<Option<Resolved<AddressingStyle>>, PythonReason> {
    let key = S3_ENDPOINT_KEYS[0];
    let Some(value) = inputs.nested(key) else {
        return Ok(None);
    };
    let style = match value {
        "auto" => AddressingStyle::Auto,
        "virtual" => AddressingStyle::Virtual,
        "path" => AddressingStyle::Path,
        _ => return Err(Field::S3Endpoint.reason()),
    };
    Ok(Some(Resolved::new(style, Source::S3(key))))
}
/// The reason a trust setting from `source` routes to Python with: the
/// command line's flag is a global, any other a network setting.
pub(super) fn trust_reason(source: Source) -> PythonReason {
    if matches!(source, Source::Flag(_)) {
        PythonReason::PythonOnlyGlobal
    } else {
        PythonReason::NetworkSetting
    }
}
pub(super) fn resolve_globals(
    settings: &mut ClientSettings,
    globals: &Globals,
) -> Result<(), PythonReason> {
    if globals.no_sign_request {
        settings.signing = Resolved::new(false, Source::Flag("--no-sign-request"));
    }
    let timeout_ms =
        timeout_milliseconds(globals.connect_timeout).ok_or(PythonReason::PythonOnlyGlobal)?;
    settings.connect_timeout_ms = Resolved::new(
        timeout_ms,
        if globals.connect_timeout.is_some() {
            Source::Flag("--cli-connect-timeout")
        } else {
            Source::Default
        },
    );
    let timeout_ms =
        timeout_milliseconds(globals.read_timeout).ok_or(PythonReason::PythonOnlyGlobal)?;
    settings.read_timeout_ms = Resolved::new(
        timeout_ms,
        if globals.read_timeout.is_some() {
            Source::Flag("--cli-read-timeout")
        } else {
            Source::Default
        },
    );
    Ok(())
}
pub(super) fn resolve_max_attempts<E: EnvMap>(
    inputs: &SettingsInputs<'_, E>,
    settings: &mut ClientSettings,
) -> Result<(), PythonReason> {
    let Some(v) = inputs.first(Field::MaxAttempts)? else {
        return Ok(());
    };
    let attempts = coerce::integer(v.value)
        .and_then(|v| u64::try_from(v).ok())
        .filter(|v| *v > 0)
        .ok_or(PythonReason::RetrySetting)?;
    settings.max_attempts = Some(Resolved::new(attempts, v.source));
    Ok(())
}
/// A checksum setting as botocore's client validates it for every command: the
/// value lowercased must be `when_supported` or `when_required`
/// (`_handle_checksum_config`), so any other value, an empty one, and a nested
/// block are Python's InvalidChecksumConfigError. An ASCII case fold is exact
/// here: the one non-ASCII character Python lowercases to an ASCII letter, the
/// Kelvin sign, folds to a `k`, which neither value holds.
pub(super) fn resolve_checksum<E: EnvMap>(
    inputs: &SettingsInputs<'_, E>,
    field: Field,
    output: &mut Resolved<ChecksumMode>,
) -> Result<(), PythonReason> {
    if let Some(v) = inputs.first(field)? {
        let mode = checksum_mode(v.value).ok_or(field.reason())?;
        *output = Resolved::new(mode, v.source);
    }
    Ok(())
}
fn checksum_mode(value: &str) -> Option<ChecksumMode> {
    [
        ("when_supported", ChecksumMode::WhenSupported),
        ("when_required", ChecksumMode::WhenRequired),
    ]
    .into_iter()
    .find_map(|(name, mode)| value.eq_ignore_ascii_case(name).then_some(mode))
}
/// botocore's `Config._validate_retry_configuration`: a mode other than
/// `standard` or `adaptive`, `legacy` included, is InvalidRetryModeError for
/// every client. An unset mode stays unset, so a customization can default
/// it; botocore's own default is `standard` (`_compute_retry_mode`).
pub(super) fn resolve_retry_mode<E: EnvMap>(
    inputs: &SettingsInputs<'_, E>,
    settings: &mut ClientSettings,
) -> Result<(), PythonReason> {
    let Some(v) = inputs.first(Field::RetryMode)? else {
        return Ok(());
    };
    let mode = match v.value {
        "standard" => RetryMode::Standard,
        "adaptive" => RetryMode::Adaptive,
        _ => return Err(PythonReason::RetrySetting),
    };
    settings.retry_mode = Some(Resolved::new(mode, v.source));
    Ok(())
}
/// TCP keepalive when the selected profile's `tcp_keepalive` is `true` in any
/// case (botocore's `_compute_socket_options`); no value is an error, and a
/// nested block, which botocore lowercases as a string, is Python's.
pub(super) fn resolve_keep_alive<E: EnvMap>(
    inputs: &SettingsInputs<'_, E>,
    settings: &mut ClientSettings,
) -> Result<(), PythonReason> {
    if let Some(v) = inputs.first(Field::TcpKeepalive)? {
        settings.keep_alive = Resolved::new(coerce::boolean(v.value), v.source);
    }
    Ok(())
}
pub(super) fn resolve_trust<E: EnvMap>(
    inputs: &SettingsInputs<'_, E>,
    settings: &mut ClientSettings,
) -> Result<(), PythonReason> {
    settings.trust = Resolved::new(default_trust(inputs.embedded_trust), Source::Default);
    let selected = if let Some(flag) = &inputs.globals.ca_bundle {
        Some(Resolved::new(OsStr::new(flag), Source::Flag("--ca-bundle")))
    } else {
        inputs.first_os(Field::CaBundle)?
    };
    let Some(value) = selected else {
        return Ok(());
    };
    if is_blank_path(value.value) {
        return Err(reject_blank(&value));
    }
    resolve_trust_file(inputs, settings, &value)
}
const fn default_trust(embedded_trust: bool) -> Trust {
    if embedded_trust {
        Trust::Embedded
    } else {
        Trust::Platform
    }
}
fn resolve_trust_file<E: EnvMap>(
    inputs: &SettingsInputs<'_, E>,
    settings: &mut ClientSettings,
    value: &Resolved<&OsStr>,
) -> Result<(), PythonReason> {
    let reason = trust_reason(value.source);
    let bytes = value.value.as_encoded_bytes();
    if bytes.len() >= TRUST_PATH_BYTES_MAX || bytes.contains(&0) {
        return Err(reason);
    }
    if open_regular_file(inputs.directory, Path::new(value.value)).is_none() {
        return Err(reason);
    }
    settings.trust = Resolved::new(Trust::File(PathBuf::from(value.value)), value.source);
    Ok(())
}
pub(super) fn reject_blank(value: &Resolved<&OsStr>) -> PythonReason {
    debug_assert!(is_blank_path(value.value));
    trust_reason(value.source)
}
pub(super) fn holds_only_certificates(directory: &Path, path: &Path) -> bool {
    open_regular_file(directory, path).is_some_and(|file| {
        let reader = std::io::BufReader::with_capacity(ca_bundle::LINE_BUFFER_BYTES, file);
        ca_bundle::holds_only_certificates(reader)
    })
}
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn open_regular_file(directory: &Path, path: &Path) -> Option<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    let base = if path.is_absolute() {
        None
    } else {
        let mut options = std::fs::OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_DIRECTORY);
        Some(options.open(directory).ok()?)
    };
    let at = base.as_ref().map_or(libc::AT_FDCWD, AsRawFd::as_raw_fd);
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let open = |flags: libc::c_int| {
        // SAFETY: `name` is a NUL-terminated string that outlives the call,
        // and `at` is `AT_FDCWD` or a descriptor `base` keeps open.
        let fd = unsafe { libc::openat(at, name.as_ptr(), flags | libc::O_CLOEXEC) };
        // SAFETY: a nonnegative `fd` is a new descriptor nothing else owns.
        (fd >= 0).then(|| std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
    };
    let probe = open(libc::O_PATH)?;
    if !probe.metadata().ok()?.is_file() {
        return None;
    }
    drop(probe);
    let file = open(libc::O_RDONLY | libc::O_NOCTTY | libc::O_NONBLOCK)?;
    file.metadata().ok()?.is_file().then_some(file)
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(super) fn open_regular_file(directory: &Path, path: &Path) -> Option<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(directory.join(path)).ok()?;
    file.metadata().ok()?.is_file().then_some(file)
}
pub(super) fn is_blank_path(path: &OsStr) -> bool {
    path.to_str().is_some_and(is_blank)
}
/// Whether `text.strip()` leaves nothing in Python, whose `str.isspace()`
/// counts the separators U+001C to U+001F as spaces.
fn is_blank(text: &str) -> bool {
    text.chars().all(awscli_config::is_space)
}
pub(super) fn resolve_proxy<E: EnvMap>(inputs: &SettingsInputs<'_, E>) -> Result<(), PythonReason> {
    let proxy = crate::proxy::Environment::from_map(inputs.env);
    if proxy.requires_python() {
        return Err(PythonReason::NetworkSetting);
    }
    if proxy.has_bypass_list()
        && !crate::endpoint_config::configured_endpoints_ascii(
            inputs.env,
            inputs.files,
            inputs.service,
            inputs.globals.endpoint_url.as_deref(),
            inputs.globals.profile.as_deref(),
        )
    {
        return Err(PythonReason::NetworkSetting);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
