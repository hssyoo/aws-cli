//! Feature helpers, JMESPath expression forms, and the features of the
//! endpoint rule set, paginators and waiters.
use super::census::Features;
use serde_json::Value as Json;

pub(crate) fn add(out: &mut Features, parts: &[&str]) {
    out.insert(parts.join("/"));
}

pub(crate) fn entries(value: Option<&Json>) -> impl Iterator<Item = (&String, &Json)> {
    value.and_then(Json::as_object).into_iter().flatten()
}

pub(crate) fn strings(value: Option<&Json>) -> impl Iterator<Item = &str> {
    let items = value.and_then(Json::as_array).into_iter().flatten();
    items.filter_map(Json::as_str)
}

/// The JSON type of a value, as a feature part.
pub(crate) fn json_kind(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "bool",
        Json::Number(number) if number.is_f64() => "float",
        Json::Number(_) => "int",
        Json::String(_) => "string",
        Json::Array(_) => "list",
        Json::Object(_) => "object",
    }
}

/// The JMESPath forms an expression uses: each operator, bracket form and
/// function by name (`fn-length`); `field` when it is a bare name.
pub(crate) fn expression_forms(text: &str) -> Vec<String> {
    let checks: [(&str, bool); 12] = [
        ("or", text.contains("||")),
        ("and", text.contains("&&")),
        ("not", text.replace("!=", "").contains('!')),
        (
            "compare",
            ["==", "!=", "<", ">"].iter().any(|op| text.contains(op)),
        ),
        ("literal", text.contains('`') || text.contains('\'')),
        ("filter", text.contains("[?")),
        ("flatten", text.contains("[]")),
        ("wildcard", text.contains("[*]") || text.contains(".*")),
        (
            "index",
            bracket_followed(text, |c| c.is_ascii_digit() || c == '-'),
        ),
        (
            "multiselect",
            bracket_followed(text, |c| c.is_alphabetic()) || text.contains('{'),
        ),
        ("dotted", text.contains('.')),
        ("pipe", text.replace("||", "").contains('|')),
    ];
    let mut forms: Vec<String> = checks
        .iter()
        .filter(|(_, used)| *used)
        .map(|(form, _)| (*form).to_owned())
        .collect();
    forms.extend(functions(text).map(|name| format!("fn-{name}")));
    if forms.is_empty() {
        forms.push("field".to_owned());
    }
    forms
}

/// Whether some `[` in `text` is followed by a character `accept` takes.
fn bracket_followed(text: &str, accept: impl Fn(char) -> bool) -> bool {
    let after = text.split('[').skip(1);
    after
        .filter_map(|rest| rest.trim_start().chars().next())
        .any(accept)
}

/// The names called in `text`: identifiers directly before `(`.
fn functions(text: &str) -> impl Iterator<Item = &str> {
    let calls = text.split('(').rev().skip(1);
    calls.filter_map(|before| {
        let start = before
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |index| index + 1);
        Some(&before[start..]).filter(|name| !name.is_empty())
    })
}

/// The features of an `endpoint-rule-set-1.json`.
pub(crate) fn rule_set_features(json: &Json, out: &mut Features) {
    for (_, parameter) in entries(json.get("parameters")) {
        for (key, value) in entries(Some(parameter)) {
            let text = value.as_str().unwrap_or("-");
            match key.as_str() {
                "documentation" => {}
                "type" => add(out, &["rules", "param", &text.to_lowercase()]),
                "builtIn" => add(out, &["rules", "builtIn", text]),
                _ => add(out, &["rules", "param-key", key]),
            }
        }
    }
    if let Some(rules) = json.get("rules") {
        rule_features(rules, out);
    }
}

/// Every function, rule type and endpoint form under `value`; a
/// `getAttr` path adds its expression forms.
fn rule_features(value: &Json, out: &mut Features) {
    match value {
        Json::Array(items) => items.iter().for_each(|item| rule_features(item, out)),
        Json::Object(object) => {
            if let Some(function) = object.get("fn").and_then(Json::as_str) {
                add(out, &["rules", "fn", function]);
                let path = object.get("argv").and_then(|argv| argv.get(1));
                if let (true, Some(path)) = (function == "getAttr", path.and_then(Json::as_str)) {
                    for form in expression_forms(path) {
                        add(out, &["rules", "fn", function, &form]);
                    }
                }
            }
            let kind = object.get("type").and_then(Json::as_str);
            if let (Some(kind), true) = (kind, object.contains_key("conditions")) {
                add(out, &["rules", "rule", kind]);
            }
            if let Some(endpoint) = object.get("endpoint").filter(|value| value.is_object()) {
                endpoint_features(endpoint, out);
            }
            object.values().for_each(|item| rule_features(item, out));
        }
        _ => {}
    }
}

fn endpoint_features(endpoint: &Json, out: &mut Features) {
    for (key, _) in entries(Some(endpoint)) {
        add(out, &["rules", "endpoint", key]);
    }
    let properties = endpoint.get("properties");
    for (key, _) in entries(properties) {
        add(out, &["rules", "property", key]);
    }
    let schemes = properties.and_then(|properties| properties.get("authSchemes"));
    for scheme in schemes.and_then(Json::as_array).into_iter().flatten() {
        for (key, value) in entries(Some(scheme)) {
            if key == "name" {
                add(out, &["rules", "authScheme", value.as_str().unwrap_or("-")]);
            } else {
                add(out, &["rules", "authScheme-key", key]);
            }
        }
    }
}

/// The features of a `paginators-1.json`: each key with its value's kind,
/// and the expression forms its strings use.
pub(crate) fn paginator_features(json: &Json, out: &mut Features) {
    for (_, paginator) in entries(json.get("pagination")) {
        for (key, value) in entries(Some(paginator)) {
            add(out, &["pages", key, json_kind(value)]);
            let texts: Vec<&str> = match value {
                Json::String(text) => vec![text],
                _ => strings(Some(value)).collect(),
            };
            for text in texts {
                for form in expression_forms(text) {
                    add(out, &["pages", key, &form]);
                }
            }
        }
    }
}

/// The features of a `waiters-2.json`: waiter keys, each matcher with each
/// state it ends in, the kind of value it expects, and the expression
/// forms of its argument.
pub(crate) fn waiter_features(json: &Json, out: &mut Features) {
    for (_, waiter) in entries(json.get("waiters")) {
        for (key, _) in entries(Some(waiter)) {
            add(out, &["wait", "key", key]);
        }
        for acceptor in waiter
            .get("acceptors")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let field = |key: &str| acceptor.get(key).and_then(Json::as_str);
            let matcher = field("matcher").unwrap_or("-");
            add(out, &["wait", matcher, field("state").unwrap_or("-")]);
            let expected = acceptor.get("expected").map_or("-", json_kind);
            add(out, &["wait", matcher, "expected", expected]);
            if let Some(argument) = field("argument") {
                for form in expression_forms(argument) {
                    add(out, &["wait", "argument", &form]);
                }
            }
            for (key, _) in entries(Some(acceptor)) {
                if !["matcher", "state", "expected", "argument"].contains(&&**key) {
                    add(out, &["wait", "acceptor-key", key]);
                }
            }
        }
    }
}
