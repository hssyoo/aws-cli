//! The one thing the legacy resolver still decides for a rule-set client:
//! whether `endpoints.json` has the FIPS or dual-stack variant asked for.
//! `ClientEndpointBridge.resolve` runs `EndpointResolver.construct_endpoint`
//! as the client is created, endpoint URL or not, and that raises
//! `EndpointVariantError` when no variant matches (regions.py
//! `_endpoint_for_partition`, `_resolve`). Nothing else of its result is
//! used.
use model::value::Value;

/// `_UNSUPPORTED_DUALSTACK_PARTITIONS`, which a dual-stack lookup skips.
const NO_DUAL_STACK: [&str; 2] = ["aws-iso", "aws-iso-b"];

/// One partition, with the service's entry there.
#[derive(Clone, Debug)]
struct Partition {
    name: String,
    regions: Vec<String>,
    /// `regionRegex` as the prefixes it allows before `\-\w+\-\d+$`;
    /// `None` for a pattern of another shape.
    prefixes: Option<Vec<String>>,
    defaults: Vec<Vec<String>>,
    service: Service,
}

/// A service's entry in a partition; `DEFAULT_SERVICE_DATA` when absent.
#[derive(Clone, Debug, Default)]
struct Service {
    endpoints: Vec<(String, Vec<Vec<String>>)>,
    defaults: Vec<Vec<String>>,
    partition_endpoint: Option<String>,
    regionalized: bool,
}

/// What the lookup found against a resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// `str(EndpointVariantError)`.
    Variant(String),
    /// A `regionRegex` the native matcher does not read.
    Pattern,
}

/// The partitions of `endpoints.json`, for the service at `prefix`.
#[derive(Clone, Debug)]
pub struct Legacy(Vec<Partition>);

/// A string value, owned.
fn owned_text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

/// A `variants` list's tag sets, each sorted, so equal sets compare equal.
fn variants(data: Option<&Value>) -> Vec<Vec<String>> {
    let Some(Value::List(variants)) = data.and_then(|data| data.get("variants")) else {
        return Vec::new();
    };
    let tags = |variant: &Value| match variant.get("tags") {
        Some(Value::List(tags)) => tags
            .iter()
            .filter_map(|tag| owned_text(Some(tag)))
            .collect(),
        _ => Vec::new(),
    };
    variants
        .iter()
        .map(|variant| {
            let mut tags: Vec<String> = tags(variant);
            tags.sort();
            tags.dedup();
            tags
        })
        .collect()
}

impl Legacy {
    pub fn parse(endpoints: &Value, prefix: &str) -> Option<Self> {
        let Some(Value::List(partitions)) = endpoints.get("partitions") else {
            return None;
        };
        Some(Self(
            partitions.iter().map(|p| partition(p, prefix)).collect(),
        ))
    }

    /// `construct_endpoint`'s variant check for `region`, which is set:
    /// each partition in order (dual-stack skipping iso and iso-b), then
    /// the `aws` partition forced, as `ClientEndpointBridge.resolve` retries.
    pub fn check(&self, region: &str, dual_stack: bool, fips: bool) -> Result<(), Refusal> {
        let mut tags: Vec<&str> = Vec::new();
        tags.extend(dual_stack.then_some("dualstack"));
        tags.extend(fips.then_some("fips"));
        if tags.is_empty() {
            return Ok(());
        }
        let mut sorted: Vec<String> = tags.iter().map(|tag| (*tag).to_owned()).collect();
        sorted.sort();
        let variant =
            |partition: &Partition, name: &str| {
                let endpoint = partition.service.endpoints.iter().find(|(n, _)| n == name);
                let found = endpoint
                    .map(|(_, v)| v.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .chain(&partition.service.defaults)
                    .chain(&partition.defaults)
                    .any(|set| *set == sorted);
                let list: Vec<String> = tags.iter().map(|tag| format!("'{tag}'")).collect();
                found.then_some(()).ok_or_else(|| Refusal::Variant(format!(
                "Unable to construct a modeled endpoint with the following variant(s) [{}]: ",
                list.join(", ")
            )))
            };
        for partition in &self.0 {
            if dual_stack && NO_DUAL_STACK.contains(&partition.name.as_str()) {
                continue;
            }
            if let Some(name) = partition.endpoint_name(region, false)? {
                return variant(partition, &name);
            }
        }
        match self.0.iter().find(|partition| partition.name == "aws") {
            Some(aws) => match aws.endpoint_name(region, true)? {
                Some(name) => variant(aws, &name),
                None => Ok(()),
            },
            None => Ok(()),
        }
    }
}

impl Partition {
    /// `_endpoint_for_partition`: the endpoint name a resolution uses in
    /// this partition, `None` when the partition does not take the region.
    fn endpoint_name(&self, region: &str, force: bool) -> Result<Option<String>, Refusal> {
        let service = &self.service;
        if service.endpoints.iter().any(|(name, _)| name == region) {
            return Ok(Some(region.to_owned()));
        }
        if !(force || self.regions.iter().any(|name| name == region) || self.matches(region)?) {
            return Ok(None);
        }
        Ok(Some(match &service.partition_endpoint {
            Some(endpoint) if !service.regionalized => endpoint.clone(),
            _ => region.to_owned(),
        }))
    }

    /// `re.match(regionRegex, region)` for the shape every partition uses:
    /// a prefix, then `-`, a word, `-` and digits.
    fn matches(&self, region: &str) -> Result<bool, Refusal> {
        let prefixes = self.prefixes.as_ref().ok_or(Refusal::Pattern)?;
        let word = |part: &str| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        };
        Ok(prefixes.iter().any(|prefix| {
            let Some(rest) = region
                .strip_prefix(prefix.as_str())
                .and_then(|r| r.strip_prefix('-'))
            else {
                return false;
            };
            matches!(rest.split_once('-'), Some((name, digits))
                if word(name) && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        }))
    }
}

fn partition(value: &Value, prefix: &str) -> Partition {
    let service = value
        .get("services")
        .and_then(|services| services.get(prefix));
    let endpoints = match service.and_then(|service| service.get("endpoints")) {
        Some(Value::Map(map)) => map
            .iter()
            // Bundled data, which `Value::from_json` reads into `Str` keys.
            .map(|(name, data)| (name.text().into_owned(), variants(Some(data))))
            .collect(),
        _ => Vec::new(),
    };
    let regions = match value.get("regions") {
        Some(Value::Map(map)) => map.keys().map(|key| key.text().into_owned()).collect(),
        _ => Vec::new(),
    };
    Partition {
        name: owned_text(value.get("partition")).unwrap_or_default(),
        regions,
        prefixes: owned_text(value.get("regionRegex")).and_then(|regex| prefixes(&regex)),
        defaults: variants(value.get("defaults")),
        service: Service {
            endpoints,
            defaults: variants(service.and_then(|service| service.get("defaults"))),
            partition_endpoint: owned_text(
                service.and_then(|service| service.get("partitionEndpoint")),
            ),
            regionalized: !matches!(
                service.and_then(|s| s.get("isRegionalized")),
                Some(Value::Bool(false))
            ),
        },
    }
}

/// The literal prefixes `^<prefix>\-\w+\-\d+$` allows: `<prefix>` of
/// lowercase letters, digits, `\-`, and groups `(a|b)` of them.
fn prefixes(regex: &str) -> Option<Vec<String>> {
    let body = regex.strip_prefix('^')?.strip_suffix(r"\-\w+\-\d+$")?;
    let mut out = vec![String::new()];
    let mut rest = body;
    while !rest.is_empty() {
        let (choices, tail): (Vec<&str>, &str) = if let Some(group) = rest.strip_prefix('(') {
            let (inside, tail) = group.split_once(')')?;
            (inside.split('|').collect(), tail)
        } else if let Some(tail) = rest.strip_prefix(r"\-") {
            (vec!["-"], tail)
        } else {
            let end = rest.find(['(', '\\']).unwrap_or(rest.len());
            (vec![&rest[..end]], &rest[end..])
        };
        let plain = |choice: &&str| {
            choice
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if choices.is_empty() || !choices.iter().all(plain) || choices.iter().any(|c| c.is_empty())
        {
            return None;
        }
        out = out
            .iter()
            .flat_map(|head| choices.iter().map(move |c| format!("{head}{c}")))
            .collect();
        rest = tail;
    }
    Some(out)
}
