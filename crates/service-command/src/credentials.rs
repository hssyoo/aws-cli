//! A model client's credentials: botocore's chain for the selected
//! profile, walked the first time anything asks for credentials: the
//! client when it signs the first attempt, or the run before it reports a
//! failure the preflight settled. Every refusal that can hand the
//! invocation to Python comes before that (`Client::prepare` asks for none),
//! so a provider with side effects, a `credential_process` above all, runs
//! once, as on the Python route.
//!
//! The model client fetches from neither the container's endpoint nor the
//! instance metadata service: [`reaches_metadata`] says, before anything
//! runs, whether the walk would ask one of them, and the preflight hands
//! such an invocation to Python (`CredentialForm`). The s3 commands' walk
//! fetches from both natively; the two walks are to merge behind the
//! `credentials` crate when the s3 commands move to the model client.
use awscli_config::ParsedFiles;
use cli_core::{SnapshotEnv, shared_config};
use client::credentials::{
    ChainCredentials, CredentialError, CredentialSource, SigningCredentials,
};
use credentials::{
    ConfigProvider, ContainerFetch, ContainerProvider, EnvProvider, InstanceMetadataProvider,
    Profile, Provider, Resolver, RoleFetcher, SharedCredentialsProvider,
};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// The selected profile's name, and whether it was selected explicitly
/// (`--profile`, `AWS_PROFILE`), which skips the environment's keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub name: String,
    pub explicit: bool,
}

impl Selected {
    fn profile(&self) -> Profile<'_> {
        if self.explicit {
            Profile::Explicit(&self.name)
        } else {
            Profile::Ambient(&self.name)
        }
    }
}

/// botocore's chain, walked on the first request for credentials.
#[derive(Debug)]
pub struct LazyChain {
    env: SnapshotEnv,
    files: ParsedFiles,
    profile: Selected,
    walked: OnceLock<Result<ChainCredentials, CredentialError>>,
}

impl LazyChain {
    /// The chain over `env` (the credential variables the router saw) and
    /// `files`, for `profile`.
    pub fn new(env: &SnapshotEnv, files: &ParsedFiles, profile: Selected) -> Self {
        Self {
            env: ::credentials::environment(env),
            files: files.clone(),
            profile,
            walked: OnceLock::new(),
        }
    }

    fn walked(&self) -> &Result<ChainCredentials, CredentialError> {
        self.walked.get_or_init(|| {
            // `reaches_metadata` refused a walk that asks either service, so
            // these stand-ins are never asked.
            let container = ContainerFetch::new(|_| Ok(None));
            let instance = RoleFetcher::new(|| Ok(None));
            ::credentials::default_resolver(
                &self.env,
                &self.files,
                self.profile.profile(),
                container,
                instance,
            )
            .load_credentials()
            .map(ChainCredentials::new)
            .map_err(|error| CredentialError(error.to_string()))
        })
    }
}

impl CredentialSource for LazyChain {
    fn credentials(&self) -> Result<Option<SigningCredentials>, CredentialError> {
        self.walked().as_ref().map_err(Clone::clone)?.credentials()
    }
    /// The provider the walk found, once it has run.
    fn method(&self) -> Option<&'static str> {
        self.walked.get()?.as_ref().ok()?.method()
    }
}

/// Whether botocore's chain for `profile` would ask the container's
/// endpoint or the instance metadata service, decided without running any
/// provider that has effects: a configured `credential_process` answers or
/// fails before either, and the other providers read the environment and
/// the files. The walk's own errors are left to the walk, which reports
/// them as botocore does.
pub fn reaches_metadata(env: &SnapshotEnv, files: &ParsedFiles, profile: &Selected) -> bool {
    if shared_config::profile_property(files, &profile.name, "credential_process").is_some() {
        return false;
    }
    let env = ::credentials::environment(env);
    let asked = Arc::new(AtomicBool::new(false));
    let noted = Arc::clone(&asked);
    let container = ContainerFetch::new(move |_| {
        noted.store(true, Ordering::Relaxed);
        Ok(None)
    });
    let reached = Arc::new(AtomicBool::new(false));
    let probed = Arc::clone(&reached);
    let instance = RoleFetcher::new(move || {
        probed.store(true, Ordering::Relaxed);
        Ok(None)
    });
    let mut providers: Vec<Box<dyn Provider + '_>> = Vec::new();
    if !profile.explicit {
        providers.push(Box::new(EnvProvider::new(&env)));
    }
    providers.push(Box::new(SharedCredentialsProvider::new(
        files,
        &profile.name,
    )));
    providers.push(Box::new(ConfigProvider::new(files, &profile.name)));
    providers.push(Box::new(ContainerProvider::new(&env, container)));
    providers.push(Box::new(InstanceMetadataProvider::new(&env, instance)));
    let found = Resolver::new(providers).load_credentials();
    matches!(found, Ok(None)) && (asked.load(Ordering::Relaxed) || reached.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests;
