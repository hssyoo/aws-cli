//! `jmespath/visitor.py`'s `TreeInterpreter`: each node evaluated as
//! Python does, with its equality, truth and ordering rules.
use crate::error::Error;
use crate::number::Number;
use crate::parser::{Comparator, Node};
use crate::runtime::{Runtime, View};
use std::cmp::Ordering;

/// What a node evaluates to: a value borrowed from the input or the
/// expression, one built, or an expression reference (`&expr`).
pub(crate) enum Val<'a, V> {
    Ref(&'a V),
    Own(V),
    Expref(&'a Node<V>),
}

impl<'a, V: Clone> Val<'a, V> {
    pub(crate) fn get(&self) -> Option<&V> {
        match self {
            Self::Ref(value) => Some(value),
            Self::Own(value) => Some(value),
            Self::Expref(_) => None,
        }
    }

    /// The value, to hold in an array, an object or the result.
    pub(crate) fn into_owned(self) -> Result<V, Error> {
        match self {
            Self::Ref(value) => Ok(value.clone()),
            Self::Own(value) => Ok(value),
            Self::Expref(_) => Err(Error::expref_value()),
        }
    }

    pub(crate) fn into_cow(self) -> Result<std::borrow::Cow<'a, V>, Error> {
        match self {
            Self::Ref(value) => Ok(std::borrow::Cow::Borrowed(value)),
            Self::Own(value) => Ok(std::borrow::Cow::Owned(value)),
            Self::Expref(_) => Err(Error::expref_value()),
        }
    }
}

pub(crate) struct Interpreter<'r, R> {
    pub(crate) runtime: &'r R,
}

type Eval<'a, V> = Result<Val<'a, V>, Error>;

impl<R: Runtime> Interpreter<'_, R> {
    pub(crate) fn view<'v>(&self, value: &'v Val<'_, R::Value>) -> Option<View<'v, R::Value>> {
        value.get().map(|value| self.runtime.view(value))
    }

    fn null<'a>(&self) -> Val<'a, R::Value> {
        Val::Own(self.runtime.null())
    }

    pub(crate) fn is_null(&self, value: &Val<'_, R::Value>) -> bool {
        matches!(self.view(value), Some(View::Null))
    }

    pub(crate) fn visit<'a>(
        &self,
        node: &'a Node<R::Value>,
        value: &'a R::Value,
    ) -> Eval<'a, R::Value> {
        let rt = self.runtime;
        Ok(match node {
            Node::Field(name) => rt.get(value, name).map_or_else(|| self.null(), Val::Ref),
            Node::Subexpression(children) | Node::IndexExpression(children) => {
                let mut result = Val::Ref(value);
                for child in children {
                    result = self.then(child, result)?;
                }
                result
            }
            Node::Pipe(left, right) => {
                let left = self.visit(left, value)?;
                self.then(right, left)?
            }
            Node::Index(index) => match rt.view(value) {
                View::Array(items) => {
                    let at = if *index < 0 {
                        index.checked_add_unsigned(items.len() as u64)
                    } else {
                        Some(*index)
                    };
                    at.and_then(|at| usize::try_from(at).ok())
                        .and_then(|at| items.get(at))
                        .map_or_else(|| self.null(), Val::Ref)
                }
                _ => self.null(),
            },
            Node::Slice(parts) => match rt.view(value) {
                View::Array(items) => Val::Own(rt.array(slice(items, *parts)?)),
                _ => self.null(),
            },
            Node::Comparator(comparator, left, right) => {
                let left = self.visit(left, value)?;
                let right = self.visit(right, value)?;
                self.compare(*comparator, &left, &right)?
            }
            Node::Current | Node::Identity => Val::Ref(value),
            Node::Expref(child) => Val::Expref(child),
            Node::Literal(literal) => Val::Ref(literal),
            Node::Function(name, args) => {
                let args = args.iter().map(|arg| self.visit(arg, value));
                self.call(name, args.collect::<Result<Vec<_>, _>>()?)?
            }
            _ => self.combine(node, value)?,
        })
    }

    /// The nodes that build a value from their children's.
    fn combine<'a>(&self, node: &'a Node<R::Value>, value: &'a R::Value) -> Eval<'a, R::Value> {
        let rt = self.runtime;
        let is_null = matches!(rt.view(value), View::Null);
        Ok(match node {
            Node::MultiSelectDict(_) | Node::MultiSelectList(_) if is_null => self.null(),
            Node::MultiSelectDict(pairs) => {
                let mut entries = Vec::with_capacity(pairs.len());
                for (key, child) in pairs {
                    entries.push((key.to_vec(), self.visit(child, value)?.into_owned()?));
                }
                Val::Own(rt.object(entries))
            }
            Node::MultiSelectList(children) => {
                let items = children
                    .iter()
                    .map(|child| self.visit(child, value)?.into_owned());
                Val::Own(rt.array(items.collect::<Result<_, _>>()?))
            }
            Node::Or(left, right) | Node::And(left, right) => {
                let matched = self.visit(left, value)?;
                if self.is_false(&matched) == matches!(node, Node::Or(..)) {
                    self.visit(right, value)?
                } else {
                    matched
                }
            }
            Node::Not(child) => {
                let result = self.visit(child, value)?;
                Val::Own(rt.boolean(self.is_not(&result)))
            }
            _ => self.project(node, value)?,
        })
    }

    /// The projections and `[]`: `None` unless the left side is a list (a
    /// dict for `*`).
    fn project<'a>(&self, node: &'a Node<R::Value>, value: &'a R::Value) -> Eval<'a, R::Value> {
        let rt = self.runtime;
        let (left, right, condition) = match node {
            Node::Projection(left, right) | Node::ValueProjection(left, right) => {
                (&**left, Some(&**right), None)
            }
            Node::FilterProjection(parts) => (&parts[0], Some(&parts[1]), Some(&parts[2])),
            Node::Flatten(child) => (&**child, None, None),
            _ => return Ok(self.null()),
        };
        let base = self.visit(left, value)?;
        let items: Vec<&R::Value> = match (self.view(&base), base.get()) {
            (Some(View::Object), Some(object)) if matches!(node, Node::ValueProjection(..)) => {
                rt.entries(object).map(|(_, item)| item).collect()
            }
            (Some(View::Array(items)), _) if !matches!(node, Node::ValueProjection(..)) => {
                items.iter().collect()
            }
            _ => return Ok(self.null()),
        };
        let mut collected = Vec::new();
        for item in items {
            let Some(right) = right else {
                match rt.view(item) {
                    View::Array(inner) => collected.extend(inner.iter().cloned()),
                    _ => collected.push(item.clone()),
                }
                continue;
            };
            if let Some(condition) = condition
                && self.is_false(&self.visit(condition, item)?)
            {
                continue;
            }
            let current = self.visit(right, item)?;
            if !self.is_null(&current) {
                collected.push(current.into_owned()?);
            }
        }
        Ok(Val::Own(rt.array(collected)))
    }

    /// `node` evaluated on `input`, the result of an earlier node.
    pub(crate) fn then<'a>(
        &self,
        node: &'a Node<R::Value>,
        input: Val<'a, R::Value>,
    ) -> Eval<'a, R::Value> {
        match input {
            Val::Ref(value) => self.visit(node, value),
            Val::Own(value) => Ok(Val::Own(self.visit(node, &value)?.into_owned()?)),
            Val::Expref(_) => Err(Error::expref_value()),
        }
    }

    /// `TreeInterpreter._is_false`: `''`, `[]`, `{}`, `None` and `False`.
    pub(crate) fn is_false(&self, value: &Val<'_, R::Value>) -> bool {
        match (self.view(value), value.get()) {
            (Some(View::Null), _) => true,
            (Some(View::Bool(flag)), _) => !flag,
            (Some(View::String(text)), _) => text.is_empty(),
            (Some(View::Array(items)), _) => items.is_empty(),
            (Some(View::Object), Some(object)) => self.runtime.entries(object).next().is_none(),
            _ => false,
        }
    }

    /// `not value`, but `False` for a zero number.
    fn is_not(&self, value: &Val<'_, R::Value>) -> bool {
        match (self.view(value), value.get()) {
            (Some(View::Number(_)), _) => false,
            (Some(View::Other), Some(other)) => !self.runtime.other_truthy(other),
            (None, _) | (Some(View::Other), None) => false,
            _ => self.is_false(value),
        }
    }

    /// The class name `type(value).__name__` gives.
    pub(crate) fn short_type(&self, value: &Val<'_, R::Value>) -> &'static str {
        match value.get() {
            Some(value) => {
                let name = self.runtime.type_name(value);
                name.rsplit('.').next().unwrap_or(name)
            }
            None => "_Expression",
        }
    }

    /// `str(value)`.
    pub(crate) fn str(&self, value: &Val<'_, R::Value>) -> String {
        value.get().map_or_else(
            || "<jmespath.visitor._Expression object>".to_owned(),
            |value| self.runtime.str(value),
        )
    }

    /// `visitor._equals`: Python's `==`, except that `0` and `1` never
    /// equal a boolean.
    pub(crate) fn equals(&self, x: &Val<'_, R::Value>, y: &Val<'_, R::Value>) -> bool {
        let special = |a: &Val<'_, R::Value>, b: &Val<'_, R::Value>| match self.view(a) {
            Some(View::Number(number)) if number.is_zero_or_one() => {
                Some(matches!(self.view(b), Some(View::Bool(_))))
            }
            _ => None,
        };
        if special(x, y).or_else(|| special(y, x)) == Some(true) {
            return false;
        }
        match (x.get(), y.get()) {
            (Some(x), Some(y)) => self.py_eq(x, y),
            _ => false,
        }
    }

    /// Python's `x == y`.
    pub(crate) fn py_eq(&self, x: &R::Value, y: &R::Value) -> bool {
        let rt = self.runtime;
        let number = |flag: bool| Number::Int(i64::from(flag));
        match (rt.view(x), rt.view(y)) {
            (View::Null, View::Null) => true,
            (View::Bool(a), View::Bool(b)) => a == b,
            (View::Bool(a), View::Number(b)) => number(a).compare(&b) == Some(Ordering::Equal),
            (View::Number(a), View::Bool(b)) => a.compare(&number(b)) == Some(Ordering::Equal),
            (View::Number(a), View::Number(b)) => a.compare(&b) == Some(Ordering::Equal),
            (View::String(a), View::String(b)) => a == b,
            (View::Array(a), View::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| self.py_eq(a, b))
            }
            (View::Object, View::Object) => {
                rt.entries(x).count() == rt.entries(y).count()
                    && rt
                        .entries(x)
                        .all(|(key, a)| rt.get(y, key).is_some_and(|b| self.py_eq(a, b)))
            }
            (View::Other, View::Other) => rt.other_eq(x, y),
            _ => false,
        }
    }

    /// Python's `x < y` (or another ordering) of two numbers or two
    /// strings; `Err` for a number against a string.
    pub(crate) fn less(
        &self,
        op: &str,
        x: &R::Value,
        y: &R::Value,
    ) -> Result<Option<Ordering>, Error> {
        match (self.runtime.view(x), self.runtime.view(y)) {
            (View::Number(a), View::Number(b)) => Ok(a.compare(&b)),
            (View::String(a), View::String(b)) => Ok(Some(a.cmp(b))),
            _ => Err(Error::unorderable(
                op,
                self.runtime.type_name(x),
                self.runtime.type_name(y),
            )),
        }
    }

    fn compare<'a>(
        &self,
        comparator: Comparator,
        x: &Val<'a, R::Value>,
        y: &Val<'a, R::Value>,
    ) -> Eval<'a, R::Value> {
        let rt = self.runtime;
        let result = match comparator {
            Comparator::Eq => self.equals(x, y),
            Comparator::Ne => !self.equals(x, y),
            _ => {
                let comparable = |v: &Val<'_, R::Value>| {
                    matches!(self.view(v), Some(View::Number(_) | View::String(_)))
                };
                let (Some(a), Some(b)) = (x.get(), y.get()) else {
                    return Ok(self.null());
                };
                if !comparable(x) || !comparable(y) {
                    return Ok(self.null());
                }
                let order = self.less(comparator.symbol(), a, b)?;
                order.is_some_and(|order| match comparator {
                    Comparator::Lt => order.is_lt(),
                    Comparator::Gt => order.is_gt(),
                    Comparator::Lte => order.is_le(),
                    _ => order.is_ge(),
                })
            }
        };
        Ok(Val::Own(rt.boolean(result)))
    }
}

/// `items[start:stop:step]`.
fn slice<V: Clone>(items: &[V], parts: [Option<i64>; 3]) -> Result<Vec<V>, Error> {
    let step = i128::from(parts[2].unwrap_or(1).max(-i64::MAX));
    if step == 0 {
        return Err(Error::new("ValueError", "slice step cannot be zero"));
    }
    let length = items.len() as i128;
    let adjust = |bound: Option<i64>, default: i128| {
        let Some(bound) = bound else { return default };
        let bound = i128::from(bound);
        if bound < 0 {
            let bound = bound + length;
            if bound < 0 {
                if step < 0 { -1 } else { 0 }
            } else {
                bound
            }
        } else if bound >= length {
            if step < 0 { length - 1 } else { length }
        } else {
            bound
        }
    };
    let (start, stop) = if step < 0 {
        (adjust(parts[0], length - 1), adjust(parts[1], -1))
    } else {
        (adjust(parts[0], 0), adjust(parts[1], length))
    };
    let mut result = Vec::new();
    let mut at = start;
    while (step > 0 && at < stop) || (step < 0 && at > stop) {
        if let Some(item) = usize::try_from(at).ok().and_then(|at| items.get(at)) {
            result.push(item.clone());
        }
        at += step;
    }
    Ok(result)
}
