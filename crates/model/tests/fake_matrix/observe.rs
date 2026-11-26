//! The observation check (G-039): a gap line may close only when a corpus
//! row runs an operation that reaches the feature. `observed.txt` maps each
//! closed line (a line of `gaps-baseline.txt` no longer in `gaps.txt`) to
//! the rows of `tests/diff/corpus/*.jsonl` that observe it. A mapped row
//! must run a fake, and the features its call reaches (the service's own,
//! its rule set's, its operation's shapes, the operation's paginator, or
//! the waiter `wait` names) must hold the feature. A feature a fake only
//! declares, on an operation no row runs, therefore cannot close its line.
//! The call's own direction counts too: output features need a success
//! outcome, error features an error outcome (status 300 or more; see
//! [`Row::scope`]), and top-level input members an argument that sends them.
use super::census::{Error, Features, Ignored, load, services};
use super::forms::{paginator_features_in, rule_set_features, waiter_features_in};
use super::walk::{Scope, service_features_in};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A corpus row on the fake models: its id, its arguments after `aws`,
/// and the statuses of its canned replies.
#[derive(Debug)]
pub(crate) struct Row {
    pub(crate) id: String,
    pub(crate) argv: Vec<String>,
    pub(crate) statuses: Vec<u64>,
}

impl Row {
    /// The part of an operation this row's call reaches. A `wait` row
    /// matches every reply against its acceptors, so each reply counts. Any
    /// other call ends at its outcome: the first reply that is not a
    /// transient status botocore retries, or the last reply when all are,
    /// so a retried 503 before a 200 is no error and a 404 after a 200 is
    /// never consumed.
    fn scope<'r>(&'r self, only: Option<&'r str>) -> Scope<'r> {
        let retried = |status: &&u64| [429, 500, 502, 503, 504].contains(*status);
        let outcome = self.statuses.iter().find(|status| !retried(status));
        let outcome = outcome.or(self.statuses.last());
        let counted: Vec<u64> = if self.argv.get(1).is_some_and(|command| command == "wait") {
            self.statuses.clone()
        } else {
            outcome.into_iter().copied().collect()
        };
        Scope {
            only,
            success: counted.iter().any(|status| (200..300).contains(status)),
            error: counted.iter().any(|status| *status >= 300),
            argv: Some(&self.argv),
        }
    }
}

/// The rows of every `.jsonl` file in `directory` whose `data_path` is
/// `tests/models`, the fakes.
pub(crate) fn rows(directory: &Path) -> Result<Vec<Row>, Error> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    files.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "jsonl")
    });
    files.sort();
    let mut rows = Vec::new();
    for path in files {
        for line in std::fs::read_to_string(&path)?
            .lines()
            .filter(|line| !line.is_empty())
        {
            let row: Json = serde_json::from_str(line)?;
            if row.get("data_path").and_then(Json::as_str) != Some("tests/models") {
                continue;
            }
            let text = |value: &Json| value.as_str().map(str::to_owned);
            let argv = row
                .get("argv")
                .and_then(Json::as_array)
                .into_iter()
                .flatten();
            let replies = row.get("responses").and_then(Json::as_array);
            let status = |reply: &Json| reply.get("status").map_or(Some(200), Json::as_u64);
            rows.push(Row {
                id: row.get("id").and_then(text).ok_or("a row without an id")?,
                argv: argv.filter_map(text).collect(),
                statuses: replies.into_iter().flatten().filter_map(status).collect(),
            });
        }
    }
    Ok(rows)
}

/// `botocore.xform_name(name, '-')` for the names fakes use: a dash
/// before a capital that follows a lowercase letter or digit, or that
/// starts a capitalized word after a capital; then lowercased.
pub(crate) fn kebab(name: &str) -> String {
    // botocore's `_special_case_transform`: a trailing `[A-Z]{2,}s`, as in
    // `ListACLs`, is one lowercase word.
    let capitals = name.strip_suffix('s').unwrap_or_default();
    let head = capitals.trim_end_matches(|c: char| c.is_ascii_uppercase());
    if capitals.len() >= head.len() + 2 {
        return format!(
            "{}-{}",
            kebab(head),
            name[head.len()..].to_ascii_lowercase()
        );
    }
    let characters: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (index, &character) in characters.iter().enumerate() {
        if index > 0 && character.is_ascii_uppercase() {
            let before = characters[index - 1];
            let next_lower = characters
                .get(index + 1)
                .is_some_and(char::is_ascii_lowercase);
            let after_word = before.is_ascii_lowercase() || before.is_ascii_digit();
            if after_word || (before.is_ascii_uppercase() && next_lower) {
                out.push('-');
            }
        }
        out.push(character.to_ascii_lowercase());
    }
    out
}

/// The name in `names` whose kebab form is `command`.
fn named<'a>(names: impl IntoIterator<Item = &'a String>, command: &str) -> Option<&'a str> {
    names
        .into_iter()
        .find(|name| kebab(name) == command)
        .map(String::as_str)
}

/// The features a row's call reaches, or why it reaches none.
pub(crate) fn reached(
    models: &BTreeMap<String, PathBuf>,
    ignored: &Ignored,
    row: &Row,
) -> Result<Features, String> {
    let [service, command, rest @ ..] = row.argv.as_slice() else {
        return Err("names no command".into());
    };
    let directory = models
        .get(service)
        .ok_or_else(|| format!("runs {service}, which is no fake"))?;
    let read = |kind: &str| load(directory, kind).map_err(|error| error.to_string());
    let model = read("service-2")?.ok_or("has no service model")?;
    let mut out = Features::new();
    let operations = model.get("operations").and_then(Json::as_object);
    if command == "wait" {
        let waiters = read("waiters-2")?.ok_or_else(|| format!("{service} has no waiters"))?;
        let names = waiters
            .get("waiters")
            .and_then(Json::as_object)
            .into_iter()
            .flatten();
        let waiter = rest
            .first()
            .and_then(|wanted| named(names.map(|(name, _)| name), wanted));
        let waiter = waiter.ok_or_else(|| format!("waits on no waiter of {service}"))?;
        let operation = waiters["waiters"][waiter]
            .get("operation")
            .and_then(Json::as_str)
            .ok_or_else(|| format!("waiter {waiter} names no operation"))?;
        service_features_in(&model, ignored, row.scope(Some(operation)), &mut out);
        waiter_features_in(&waiters, Some(waiter), &mut out);
    } else {
        let keys = operations.into_iter().flatten().map(|(name, _)| name);
        let operation = named(keys, command)
            .ok_or_else(|| format!("runs `{command}`, which no operation of {service} is"))?;
        service_features_in(&model, ignored, row.scope(Some(operation)), &mut out);
        // An operation with endpoint discovery first calls the service's
        // endpoint operation, as botocore's discovery handler does.
        let discovers = operations.and_then(|all| all.get(operation)?.get("endpointdiscovery"));
        let endpoint = operations.into_iter().flatten();
        let endpoint = endpoint.filter(|(_, found)| found.get("endpointoperation").is_some());
        for (name, _) in endpoint.filter(|_| discovers.is_some()) {
            service_features_in(&model, ignored, row.scope(Some(name)), &mut out);
        }
        let paged = row.scope(None).success;
        if let (Some(pages), true) = (read("paginators-1")?, paged) {
            paginator_features_in(&pages, Some(operation), &mut out);
        }
    }
    if let Some(rules) = read("endpoint-rule-set-1")? {
        rule_set_features(&rules, &mut out);
    }
    Ok(out)
}

/// The fakes under `root`, by service name.
pub(crate) fn fakes(root: &Path) -> Result<BTreeMap<String, PathBuf>, Error> {
    Ok(services(root)?.into_iter().collect())
}

/// `observed.txt`'s `feature row-id` pairs, `#` comments and blank lines
/// skipped.
pub(crate) fn mapped(text: &str) -> Result<Vec<(&str, &str)>, Error> {
    let lines = text.lines().map(str::trim);
    let lines = lines.filter(|line| !line.is_empty() && !line.starts_with('#'));
    lines
        .map(
            |line| match line.split_whitespace().collect::<Vec<_>>()[..] {
                [feature, row] => Ok((feature, row)),
                _ => Err(format!("observed.txt line `{line}` is not `feature row-id`").into()),
            },
        )
        .collect()
}

/// One line per problem: a closed gap no mapped row observes, a mapped row
/// that does not reach its feature or does not exist, an entry for a line
/// that is not closed, and an unsorted or repeated entry. `reach` holds
/// each row's reached features, or why it reaches none.
pub(crate) fn observation_problems(
    closed: &BTreeSet<&str>,
    map: &[(&str, &str)],
    reach: &BTreeMap<String, Result<Features, String>>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for pair in map.windows(2) {
        if pair[0] >= pair[1] {
            let (feature, row) = pair[1];
            problems.push(format!(
                "observed.txt line {feature} {row} is out of order or repeated"
            ));
        }
    }
    let mut observers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for &(feature, row) in map {
        if !closed.contains(feature) {
            problems.push(format!(
                "observed.txt line {feature} {row} is stale: {feature} is no closed gap line"
            ));
            continue;
        }
        match reach.get(row) {
            None => problems.push(format!("observed.txt: row {row} is in no corpus file")),
            Some(Err(why)) => problems.push(format!("observed.txt: row {row} {why}")),
            Some(Ok(features)) if !features.contains(feature) => problems.push(format!(
                "observed.txt: row {row} does not reach {feature}: its call never touches a \
                 shape, operation or file that carries it"
            )),
            Some(Ok(_)) => observers.entry(feature).or_default().push(row),
        }
    }
    for feature in closed
        .iter()
        .filter(|feature| !observers.contains_key(*feature))
    {
        let candidates: Vec<&str> = reach
            .iter()
            .filter(|(_, found)| found.as_ref().is_ok_and(|found| found.contains(*feature)))
            .map(|(row, _)| row.as_str())
            .take(3)
            .collect();
        problems.push(format!(
            "gap line {feature} is closed, but no row observed.txt maps to it observes it; rows \
             that reach it: {candidates:?} (none: keep the line in gaps.txt)"
        ));
    }
    problems
}
