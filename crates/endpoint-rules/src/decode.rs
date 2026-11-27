//! `endpoint-rule-set-1.json` → [`RuleSet`]. The decoder accepts what the
//! CRT engine's parser accepts, keys it ignores included, and refuses
//! besides what that engine would only fail on when it reached it: a
//! reference to a name not in scope, an assignment that shadows a name, a
//! call with the wrong number of arguments, a `getAttr` path that is not a
//! literal, a number that is not an integer.
use crate::error::DecodeError;
use crate::ir::{
    Condition, EvalValue, Expr, FnId, ParamType, ParameterDef, PathPart, Rule, RuleSet, Slot,
    TemplatePart,
};
use crate::template;
use serde_json::{Map, Value};

type Result<T> = std::result::Result<T, DecodeError>;

fn fail<T>(detail: impl Into<String>) -> Result<T> {
    Err(DecodeError::rule_set(detail.into()))
}

/// Decodes a rule set document.
pub fn decode_rule_set(json: &[u8]) -> Result<RuleSet> {
    let root: Value =
        serde_json::from_slice(json).or_else(|error| fail(format!("not JSON: {error}")))?;
    let root = object(&root, "the rule set")?;
    let version = text(root.get("version"), "version")?;
    optional_text(root.get("serviceId"), "serviceId")?;
    let params = parameters(root.get("parameters"))?;
    let mut decoder = Decoder {
        names: params.iter().map(|p| p.name.clone()).collect(),
        scope_len: params.len(),
    };
    let rules = decoder.rules(root.get("rules"))?;
    Ok(RuleSet {
        version: version.into(),
        params: params.into(),
        rules,
        scope_len: slot_index(decoder.scope_len)?,
    })
}

fn object<'v>(value: &'v Value, what: &str) -> Result<&'v Map<String, Value>> {
    value
        .as_object()
        .map_or_else(|| fail(format!("{what} is not an object")), Ok)
}

fn text<'v>(value: Option<&'v Value>, what: &str) -> Result<&'v str> {
    value
        .and_then(Value::as_str)
        .map_or_else(|| fail(format!("{what} is not a string")), Ok)
}

fn optional_text<'v>(value: Option<&'v Value>, what: &str) -> Result<Option<&'v str>> {
    value.map(|value| text(Some(value), what)).transpose()
}

fn array<'v>(value: Option<&'v Value>, what: &str) -> Result<&'v [Value]> {
    value.and_then(Value::as_array).map_or_else(
        || fail(format!("{what} is not an array")),
        |a| Ok(a.as_slice()),
    )
}

fn slot_index(index: usize) -> Result<u16> {
    u16::try_from(index).or_else(|_| fail("more than 65535 names in scope"))
}

fn parameters(value: Option<&Value>) -> Result<Vec<ParameterDef>> {
    let defs = object(value.unwrap_or(&Value::Null), "parameters")?;
    let mut params = Vec::with_capacity(defs.len());
    for (index, (name, def)) in defs.iter().enumerate() {
        params.push(parameter(name, def, Slot(slot_index(index)?))?);
    }
    Ok(params)
}

fn parameter(name: &str, def: &Value, slot: Slot) -> Result<ParameterDef> {
    let at = |key: &str| format!("parameter {name}: {key}");
    let def = object(def, &at("definition"))?;
    let ty = match text(def.get("type"), &at("type"))?
        .to_ascii_lowercase()
        .as_str()
    {
        "string" => ParamType::String,
        "boolean" => ParamType::Boolean,
        "stringarray" => ParamType::StringArray,
        other => return fail(at(&format!("unknown type {other:?}"))),
    };
    optional_text(def.get("documentation"), &at("documentation"))?;
    let builtin = optional_text(def.get("builtIn"), &at("builtIn"))?;
    let required = match def.get("required") {
        None => false,
        Some(value) => value.as_bool().map_or_else(|| fail(at("required")), Ok)?,
    };
    let default = match def.get("default") {
        None => None,
        Some(value) => Some(default_value(ty, value).map_or_else(|| fail(at("default")), Ok)?),
    };
    if let Some(deprecated) = def.get("deprecated") {
        optional_text(deprecated.get("message"), &at("deprecated message"))?;
        optional_text(deprecated.get("since"), &at("deprecated since"))?;
    }
    Ok(ParameterDef {
        name: name.into(),
        ty,
        builtin: builtin.map(Into::into),
        required,
        default,
        slot,
    })
}

/// A default of the parameter's type, or `None`.
fn default_value(ty: ParamType, value: &Value) -> Option<EvalValue> {
    match (ty, value) {
        (ParamType::String, Value::String(text)) => Some(EvalValue::str(text)),
        (ParamType::Boolean, Value::Bool(flag)) => Some(EvalValue::Bool(*flag)),
        (ParamType::StringArray, Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(EvalValue::str))
            .collect::<Option<Vec<_>>>()
            .map(|items| EvalValue::Array(items.into())),
        _ => None,
    }
}

/// The names in scope, innermost last: the parameters, then each enclosing
/// rule's assignments.
struct Decoder {
    names: Vec<Box<str>>,
    scope_len: usize,
}

impl Decoder {
    fn slot(&self, name: &str) -> Result<Slot> {
        match self.names.iter().rposition(|known| &**known == name) {
            Some(index) => Ok(Slot(slot_index(index)?)),
            None => fail(format!("{name:?} is not in scope")),
        }
    }

    fn bind(&mut self, name: &str) -> Result<Slot> {
        if self.names.iter().any(|known| &**known == name) {
            return fail(format!("assignment to {name:?} shadows a name in scope"));
        }
        let slot = Slot(slot_index(self.names.len())?);
        self.names.push(name.into());
        self.scope_len = self.scope_len.max(self.names.len());
        Ok(slot)
    }

    fn rules(&mut self, value: Option<&Value>) -> Result<Box<[Rule]>> {
        array(value, "rules")?
            .iter()
            .map(|rule| self.rule(rule))
            .collect()
    }

    fn rule(&mut self, value: &Value) -> Result<Rule> {
        let rule = object(value, "a rule")?;
        let kind = text(rule.get("type"), "a rule's type")?.to_ascii_lowercase();
        let mark = self.names.len();
        let when = array(rule.get("conditions"), "a rule's conditions")?
            .iter()
            .map(|condition| self.condition(condition))
            .collect::<Result<Box<[_]>>>()?;
        let decoded = match kind.as_str() {
            "endpoint" => self.endpoint(when, rule.get("endpoint"))?,
            "error" => Rule::Error {
                when,
                message: self.result_expr(rule.get("error"), "an error rule's error")?,
            },
            "tree" => Rule::Tree {
                when,
                rules: self.rules(rule.get("rules"))?,
            },
            other => return fail(format!("unknown rule type {other:?}")),
        };
        optional_text(rule.get("documentation"), "a rule's documentation")?;
        self.names.truncate(mark);
        Ok(decoded)
    }

    fn condition(&mut self, value: &Value) -> Result<Condition> {
        let condition = object(value, "a condition")?;
        let call = self.call(condition)?;
        let assign = match optional_text(condition.get("assign"), "assign")? {
            None | Some("") => None,
            Some(name) => Some(self.bind(name)?),
        };
        Ok(Condition { call, assign })
    }

    fn endpoint(&self, when: Box<[Condition]>, value: Option<&Value>) -> Result<Rule> {
        let endpoint = object(value.unwrap_or(&Value::Null), "an endpoint rule's endpoint")?;
        let url = match endpoint.get("url") {
            None => Expr::Lit(EvalValue::str("")),
            url => self.result_expr(url, "an endpoint's url")?,
        };
        let properties = endpoint
            .get("properties")
            .map(|value| {
                let blob = serde_json::to_string(value).or_else(|e| fail(e.to_string()))?;
                template::split(&blob, true, |text| self.reference(text))
            })
            .transpose()?;
        let headers = match endpoint.get("headers") {
            None => Box::default(),
            Some(headers) => object(headers, "an endpoint's headers")?
                .iter()
                .map(|(name, values)| {
                    let values = array(Some(values), "a header's values")?;
                    let values = values.iter().map(|value| self.expr(value));
                    Ok((name.as_str().into(), values.collect::<Result<_>>()?))
                })
                .collect::<Result<_>>()?,
        };
        Ok(Rule::Endpoint {
            when,
            url,
            properties,
            headers,
        })
    }

    /// An endpoint's url or an error rule's error: a string, a reference
    /// or a call.
    fn result_expr(&self, value: Option<&Value>, what: &str) -> Result<Expr> {
        match value {
            Some(Value::String(text)) => self.string_expr(text),
            Some(value @ Value::Object(_)) => self.expr(value),
            _ => fail(format!("{what} is not a string, reference or call")),
        }
    }

    fn expr(&self, value: &Value) -> Result<Expr> {
        match value {
            Value::String(text) => self.string_expr(text),
            Value::Bool(flag) => Ok(Expr::Lit(EvalValue::Bool(*flag))),
            Value::Number(number) => number.as_i64().map_or_else(
                || fail(format!("{number} is not an integer")),
                |n| Ok(Expr::Lit(EvalValue::Int(n))),
            ),
            Value::Array(items) => Ok(Expr::Array(
                items
                    .iter()
                    .map(|item| self.expr(item))
                    .collect::<Result<_>>()?,
            )),
            Value::Object(fields) => match fields.get("ref") {
                Some(name) => Ok(Expr::Ref(self.slot(text(Some(name), "a reference")?)?)),
                None => self.call(fields),
            },
            Value::Null => fail("null is not an expression"),
        }
    }

    fn string_expr(&self, text: &str) -> Result<Expr> {
        if text.contains('{') {
            let parts = template::split(text, false, |inner| self.reference(inner))?;
            Ok(Expr::Template(parts))
        } else {
            Ok(Expr::Lit(EvalValue::str(text)))
        }
    }

    /// A template's `name` or `name#path`; like the CRT engine, a second
    /// `#` ends the path.
    fn reference(&self, inner: &str) -> Result<TemplatePart> {
        let mut pieces = inner.split('#');
        let name = pieces.next().unwrap_or_default();
        if name.is_empty() {
            return fail(format!("template reference {inner:?} has no name"));
        }
        let slot = self.slot(name)?;
        Ok(match pieces.next() {
            None => TemplatePart::Var(slot),
            Some(at) => TemplatePart::Path(slot, path(at)?),
        })
    }

    fn call(&self, call: &Map<String, Value>) -> Result<Expr> {
        let name = text(call.get("fn"), "a call's fn")?;
        let argv = array(call.get("argv"), "a call's argv")?;
        let (id, arity) = match name {
            "getAttr" => return self.get_attr(argv),
            name => {
                FnId::lookup(name).map_or_else(|| fail(format!("unknown function {name:?}")), Ok)?
            }
        };
        if argv.len() != arity {
            return fail(format!(
                "{name} takes {arity} arguments, not {}",
                argv.len()
            ));
        }
        let args = argv
            .iter()
            .map(|arg| self.expr(arg))
            .collect::<Result<_>>()?;
        Ok(Expr::Fn(id, args))
    }

    fn get_attr(&self, argv: &[Value]) -> Result<Expr> {
        match argv {
            [target, Value::String(at)] if !at.contains('{') => {
                Ok(Expr::GetAttr(Box::new(self.expr(target)?), path(at)?))
            }
            _ => fail("getAttr takes a value and a literal path"),
        }
    }
}

/// A `getAttr` path: `.`-separated names, the last of which may end in
/// `[index]` (or be only `[index]`). Paths the CRT engine would walk
/// differently for an array than for a record are refused.
fn path(text: &str) -> Result<Box<[PathPart]>> {
    let mut parts = Vec::new();
    let mut segments = text.split('.').peekable();
    while let Some(segment) = segments.next() {
        let (name, index) = match segment.split_once('[') {
            None => (segment, None),
            Some((name, rest)) => (name, Some(index(rest, segments.peek().is_none())?)),
        };
        if !name.is_empty() {
            parts.push(PathPart::Key(name.into()));
        } else if index.is_none() {
            return fail(format!("empty segment in path {text:?}"));
        }
        parts.extend(index.map(PathPart::Index));
    }
    Ok(parts.into())
}

/// `rest` of `name[rest`: an index and `]`, in the path's last segment.
fn index(rest: &str, last: bool) -> Result<i64> {
    let digits = rest
        .strip_suffix(']')
        .filter(|d| last && !d.starts_with('+'));
    digits
        .and_then(|digits| digits.parse().ok())
        .map_or_else(|| fail(format!("bad path index [{rest}")), Ok)
}
