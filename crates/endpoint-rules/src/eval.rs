//! The tree evaluator, as the CRT engine walks a rule set
//! (`endpoints_rule_engine.c`): rules in order, each rule's conditions
//! in order until one is unset or false, an assignment only from a
//! truthy condition, and a matched tree final: when none of its rules
//! matches, evaluation fails rather than going on to the tree's
//! siblings.
use crate::error::EvalError;
use crate::ir::{Condition, EvalValue, Expr, Header, PathPart, Rule, RuleSet, TemplatePart};
use crate::{Endpoint, Library, Outcome, Params, stdlib};

type Result<T> = std::result::Result<T, EvalError>;

pub(crate) fn evaluate(rules: &RuleSet, args: &Params, lib: &Library<'_>) -> Result<Outcome> {
    let mut scope = vec![None; usize::from(rules.scope_len)];
    for param in &rules.params {
        let value = args.get(param.slot).cloned();
        // Like the CRT engine, only a required parameter takes its default.
        let value = match (value, &param.default) {
            (None, Some(default)) if param.required => Some(default.clone()),
            (None, None) if param.required => {
                return Err(EvalError::new(format!(
                    "required parameter {} is unset",
                    param.name
                )));
            }
            (value, _) => value,
        };
        scope[param.slot.index()] = value;
    }
    if rules.rules.is_empty() {
        return Err(EvalError::new("the rule set has no rules"));
    }
    Eval { scope, lib }
        .rules(&rules.rules)?
        .ok_or_else(exhausted)
}

fn exhausted() -> EvalError {
    EvalError::new("no rule matched")
}

struct Eval<'a> {
    scope: Vec<Option<EvalValue>>,
    lib: &'a Library<'a>,
}

impl Eval<'_> {
    fn rules(&mut self, rules: &[Rule]) -> Result<Option<Outcome>> {
        for rule in rules {
            if let Some(outcome) = self.rule(rule)? {
                return Ok(Some(outcome));
            }
        }
        Ok(None)
    }

    fn rule(&mut self, rule: &Rule) -> Result<Option<Outcome>> {
        if !self.conditions(rule.when())? {
            return Ok(None);
        }
        Ok(Some(match rule {
            Rule::Endpoint {
                url,
                properties,
                headers,
                ..
            } => Outcome::Endpoint(self.endpoint(url, properties.as_deref(), headers)?),
            Rule::Error { message, .. } => Outcome::Error(self.string(message)?.to_string()),
            Rule::Tree { rules, .. } => self.rules(rules)?.ok_or_else(exhausted)?,
        }))
    }

    /// Whether every condition holds; when one does not, the rule's
    /// assignments are undone.
    fn conditions(&mut self, when: &[Condition]) -> Result<bool> {
        for (done, condition) in when.iter().enumerate() {
            let value = self.expr(&condition.call)?;
            if !value.as_ref().is_some_and(EvalValue::truthy) {
                for slot in when[..done].iter().filter_map(|c| c.assign) {
                    self.scope[slot.index()] = None;
                }
                return Ok(false);
            }
            if let Some(slot) = condition.assign {
                self.scope[slot.index()] = value;
            }
        }
        Ok(true)
    }

    fn endpoint(
        &self,
        url: &Expr,
        properties: Option<&[TemplatePart]>,
        headers: &[Header],
    ) -> Result<Endpoint> {
        let url = self.string(url)?.to_string();
        let properties = match properties {
            None => serde_json::Value::Object(serde_json::Map::new()),
            Some(parts) => serde_json::from_str(&self.render(parts, true)?)
                .map_err(|error| EvalError::new(format!("properties: {error}")))?,
        };
        let headers = headers
            .iter()
            .map(|(name, values)| {
                let values = values
                    .iter()
                    .map(|value| Ok(self.string(value)?.to_string()));
                Ok((name.to_string(), values.collect::<Result<_>>()?))
            })
            .collect::<Result<_>>()?;
        Ok(Endpoint {
            url,
            properties,
            headers,
        })
    }

    fn string(&self, expr: &Expr) -> Result<std::sync::Arc<str>> {
        match self.expr(expr)? {
            Some(EvalValue::Str(text)) => Ok(text),
            _ => Err(EvalError::new("a url, error or header is not a string")),
        }
    }

    fn expr(&self, expr: &Expr) -> Result<Option<EvalValue>> {
        match expr {
            Expr::Lit(value) => Ok(Some(value.clone())),
            Expr::Template(parts) => Ok(Some(EvalValue::Str(self.render(parts, false)?.into()))),
            Expr::Ref(slot) => Ok(self.scope[slot.index()].clone()),
            Expr::Fn(id, args) => {
                let args = args
                    .iter()
                    .map(|arg| self.expr(arg))
                    .collect::<Result<Vec<_>>>()?;
                stdlib::call(*id, &args, self.lib.partitions)
            }
            Expr::GetAttr(target, path) => get_attr(self.expr(target)?.as_ref(), path),
            Expr::Array(items) => {
                let items = items.iter().map(|item| {
                    self.expr(item)?
                        .ok_or_else(|| EvalError::new("an array element is unset"))
                });
                Ok(Some(EvalValue::Array(
                    items.collect::<Result<Vec<_>>>()?.into(),
                )))
            }
        }
    }

    /// A template's text. In `json` mode a value holding `"` fails: the
    /// CRT engine would splice it into the JSON unescaped.
    fn render(&self, parts: &[TemplatePart], json: bool) -> Result<String> {
        let mut out = String::new();
        for part in parts {
            let value = match part {
                TemplatePart::Lit(text) => {
                    out.push_str(text);
                    continue;
                }
                TemplatePart::Var(slot) => self.scope[slot.index()].clone(),
                TemplatePart::Path(slot, path) => {
                    get_attr(self.scope[slot.index()].as_ref(), path)?
                }
            };
            match value {
                Some(EvalValue::Str(text)) if !(json && text.contains('"')) => out.push_str(&text),
                _ => return Err(EvalError::new("a template reference is not a string")),
            }
        }
        Ok(out)
    }
}

/// `getAttr`: names through records, then an index, which ends the path.
/// Like the CRT engine: a missing name fails, an index past the end (or
/// into a value that is not an array) is unset, and a negative index
/// counts from the end.
fn get_attr(target: Option<&EvalValue>, path: &[PathPart]) -> Result<Option<EvalValue>> {
    let fail = |what: &str| EvalError::new(format!("getAttr: {what}"));
    let mut value = match target {
        Some(value @ (EvalValue::Record(_) | EvalValue::Array(_))) => value,
        _ => return Err(fail("not a record or array")),
    };
    for part in path {
        match part {
            PathPart::Key(key) => value = value.field(key).ok_or_else(|| fail("no such field"))?,
            PathPart::Index(index) => {
                let items: &[EvalValue] = match value {
                    EvalValue::Array(items) => items,
                    _ => &[],
                };
                let at = if *index < 0 {
                    let from_end = i64::try_from(items.len()).unwrap_or(i64::MAX) + index;
                    usize::try_from(from_end).map_err(|_| fail("index before the start"))?
                } else {
                    usize::try_from(*index).unwrap_or(usize::MAX)
                };
                return Ok(items.get(at).cloned());
            }
        }
    }
    Ok(Some(value.clone()))
}
