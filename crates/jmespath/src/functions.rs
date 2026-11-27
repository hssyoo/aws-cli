//! `jmespath/functions.py`: the built-in functions, their signatures and
//! the type checks `Functions._validate_arguments` makes, in its order:
//! the name, the arity, then each argument the signature lists.
use crate::error::Error;
use crate::eval::{Interpreter, Val};
use crate::number::{self, Number};
use crate::parser::Node;
use crate::runtime::{Runtime, View};
use std::cmp::Ordering;

/// A function's signature: each argument's allowed types (none: any),
/// and whether the last repeats.
struct Signature {
    args: &'static [&'static [&'static str]],
    variadic: bool,
}

const fn fixed(args: &'static [&'static [&'static str]]) -> Signature {
    Signature {
        args,
        variadic: false,
    }
}

const NUMBER: &[&str] = &["number"];
const STRING: &[&str] = &["string"];
const ANY: &[&str] = &[];
const EXPREF: &[&str] = &["expref"];
const ARRAY: &[&str] = &["array"];
const OBJECT: &[&str] = &["object"];

fn signature(name: &str) -> Option<Signature> {
    Some(match name {
        "abs" | "ceil" | "floor" => fixed(&[NUMBER]),
        "avg" | "sum" => fixed(&[&["array-number"]]),
        "not_null" => Signature {
            args: &[ANY],
            variadic: true,
        },
        "to_array" | "to_string" | "to_number" | "type" => fixed(&[ANY]),
        "contains" => fixed(&[&["array", "string"], ANY]),
        "length" => fixed(&[&["string", "array", "object"]]),
        "ends_with" | "starts_with" => fixed(&[STRING, STRING]),
        "reverse" => fixed(&[&["array", "string"]]),
        "join" => fixed(&[STRING, &["array-string"]]),
        "map" => fixed(&[EXPREF, ARRAY]),
        "max" | "min" => fixed(&[&["array-number", "array-string"]]),
        "merge" => Signature {
            args: &[OBJECT],
            variadic: true,
        },
        "sort" => fixed(&[&["array-string", "array-number"]]),
        "keys" | "values" => fixed(&[OBJECT]),
        "sort_by" | "min_by" | "max_by" => fixed(&[ARRAY, EXPREF]),
        _ => return None,
    })
}

/// `REVERSE_TYPES_MAP`: the Python classes of a JMESPath type.
fn classes(kind: &str) -> &'static [&'static str] {
    match kind {
        "boolean" => &["bool"],
        "array" => &["list", "_Projection"],
        "object" => &["dict", "OrderedDict"],
        "null" => &["NoneType"],
        "string" => &["unicode", "str"],
        "number" => &["float", "int", "long"],
        _ => &["_Expression"],
    }
}

/// `TYPES_MAP.get(class, 'unknown')`.
fn jmespath_type(class: &str) -> &'static str {
    match class {
        "bool" => "boolean",
        "list" | "_Projection" => "array",
        "dict" | "OrderedDict" => "object",
        "NoneType" => "null",
        "unicode" | "str" => "string",
        "float" | "int" | "long" => "number",
        "_Expression" => "expref",
        _ => "unknown",
    }
}

type Eval<'a, V> = Result<Val<'a, V>, Error>;

impl<R: Runtime> Interpreter<'_, R> {
    /// `Functions.call_function(name, args)`.
    pub(crate) fn call<'a>(&self, name: &str, args: Vec<Val<'a, R::Value>>) -> Eval<'a, R::Value> {
        let signature = signature(name).ok_or_else(|| Error::unknown_function(name))?;
        let expected = signature.args.len();
        if (signature.variadic && args.len() < expected)
            || (!signature.variadic && args.len() != expected)
        {
            return Err(Error::arity(expected, args.len(), name, signature.variadic));
        }
        for (arg, types) in args.iter().zip(signature.args) {
            if !types.is_empty() {
                self.check(name, arg, types)?;
            }
        }
        self.dispatch(name, args)
    }

    /// `Functions._type_check_single`.
    fn check(&self, name: &str, arg: &Val<'_, R::Value>, types: &[&str]) -> Result<(), Error> {
        let class = self.short_type(arg);
        let mut allowed: Vec<&str> = Vec::new();
        let mut subtypes: Vec<&[&str]> = Vec::new();
        for kind in types {
            let (kind, subtype) = kind
                .split_once('-')
                .map_or((*kind, None), |(kind, sub)| (kind, Some(sub)));
            subtypes.extend(subtype.map(classes));
            allowed.extend(classes(kind));
        }
        if !allowed.contains(&class) {
            return Err(Error::invalid_type(
                name,
                &self.str(arg),
                jmespath_type(class),
                types,
            ));
        }
        let Some(View::Array(items)) = self.view(arg) else {
            return Ok(());
        };
        let wrong = |item: &R::Value| {
            let class = self.short_type(&Val::Ref(item));
            Error::invalid_type(name, &self.runtime.str(item), class, types)
        };
        let allowed = match subtypes.as_slice() {
            [] => return Ok(()),
            [only] => *only,
            several => match items.first() {
                None => return Ok(()),
                Some(first) => {
                    let class = self.short_type(&Val::Ref(first));
                    *several
                        .iter()
                        .find(|subtypes| subtypes.contains(&class))
                        .ok_or_else(|| wrong(first))?
                }
            },
        };
        match items
            .iter()
            .find(|item| !allowed.contains(&self.short_type(&Val::Ref(item))))
        {
            Some(item) => Err(wrong(item)),
            None => Ok(()),
        }
    }

    fn dispatch<'a>(&self, name: &str, mut args: Vec<Val<'a, R::Value>>) -> Eval<'a, R::Value> {
        let first = args.remove(0);
        match name {
            "abs" | "ceil" | "floor" | "avg" | "sum" | "max" | "min" | "sort" => {
                self.numeric(name, &first).map(Val::Own)
            }
            "not_null" | "to_array" | "to_string" | "to_number" | "type" | "merge" => {
                self.convert(name, first, args)
            }
            "map" | "sort_by" | "min_by" | "max_by" => self.higher(name, &first, &args[0]),
            _ => self.collection(name, &first, &args).map(Val::Own),
        }
    }

    /// The functions of numbers, and those that order a list.
    fn numeric(&self, name: &str, first: &Val<'_, R::Value>) -> Result<R::Value, Error> {
        let rt = self.runtime;
        Ok(match (name, self.view(first)) {
            ("abs", Some(View::Number(n))) => rt.number(n.abs()),
            ("ceil" | "floor", Some(View::Number(n))) => rt.number(n.round(name == "ceil")?),
            ("avg", Some(View::Array([]))) => rt.null(),
            ("avg", Some(View::Array(items))) => {
                let total = number::sum(&self.numbers(items))?.to_f64()?;
                rt.number(Number::Float(total / items.len() as f64))
            }
            ("sum", Some(View::Array(items))) => rt.number(number::sum(&self.numbers(items))?),
            ("max" | "min", Some(View::Array(items))) => {
                let op = if name == "max" { ">" } else { "<" };
                let best = self.best(items, op, |item| Ok(item.clone()))?;
                best.unwrap_or_else(|| rt.null())
            }
            ("sort", Some(View::Array(items))) => {
                let mut items = items.to_vec();
                crate::sort::sort(&mut items, |a, b| self.lt(a, b));
                rt.array(items)
            }
            _ => rt.null(),
        })
    }

    /// The conversions, `not_null` and `merge`.
    fn convert<'a>(
        &self,
        name: &str,
        first: Val<'a, R::Value>,
        rest: Vec<Val<'a, R::Value>>,
    ) -> Eval<'a, R::Value> {
        let rt = self.runtime;
        let own = |value| Ok(Val::Own(value));
        let (view, value) = (self.view(&first), first.get());
        match (name, view, value) {
            ("not_null", ..) => Ok(std::iter::once(first)
                .chain(rest)
                .find(|arg| !self.is_null(arg))
                .unwrap_or_else(|| Val::Own(rt.null()))),
            ("to_array" | "to_string", Some(View::Array(_)), _) if name == "to_array" => Ok(first),
            ("to_string", Some(View::String(_)), _) => Ok(first),
            ("to_array", ..) => own(rt.array(vec![first.into_owned()?])),
            ("to_string", _, Some(value)) => own(rt.string(rt.dumps(value).into_bytes())),
            ("to_number", Some(View::Array(_) | View::Object | View::Bool(_) | View::Null), _) => {
                own(rt.null())
            }
            ("to_number", Some(View::Number(_)), _) => Ok(first),
            ("to_number", _, Some(value)) => own(rt
                .to_number(value)?
                .map_or_else(|| rt.null(), |number| rt.number(number))),
            ("to_number", ..) => Err(Error::type_error(
                "int() argument must be a string, a bytes-like object or a real number, \
                 not '_Expression'",
            )),
            ("type", Some(view), _) => own(match view {
                View::String(_) => rt.string(b"string".to_vec()),
                View::Bool(_) => rt.string(b"boolean".to_vec()),
                View::Array(_) => rt.string(b"array".to_vec()),
                View::Object => rt.string(b"object".to_vec()),
                View::Number(_) => rt.string(b"number".to_vec()),
                View::Null => rt.string(b"null".to_vec()),
                View::Other => rt.null(),
            }),
            ("merge", ..) => {
                let mut merged = Vec::new();
                for arg in std::iter::once(&first).chain(&rest) {
                    self.update(&mut merged, arg)?;
                }
                own(rt.object(merged))
            }
            ("type", ..) => own(rt.null()),
            _ => Err(Error::expref_value()),
        }
    }

    /// The functions of strings, arrays and objects.
    fn collection(
        &self,
        name: &str,
        first: &Val<'_, R::Value>,
        rest: &[Val<'_, R::Value>],
    ) -> Result<R::Value, Error> {
        let rt = self.runtime;
        let string = |text: &[u8]| rt.string(text.to_vec());
        let int = |count: usize| rt.number(Number::Int(count as i64));
        let second = rest.first().and_then(|arg| self.view(arg));
        Ok(match (name, self.view(first), second) {
            ("contains", Some(View::Array(items)), _) => {
                let search = rest[0].get();
                rt.boolean(
                    search.is_some_and(|search| items.iter().any(|item| self.py_eq(item, search))),
                )
            }
            ("contains", Some(View::String(text)), Some(View::String(search))) => {
                rt.boolean(search.is_empty() || text.windows(search.len()).any(|w| w == search))
            }
            ("contains", ..) => {
                let class = rest[0]
                    .get()
                    .map_or("_Expression", |search| rt.type_name(search));
                let message = format!("'in <string>' requires string as left operand, not {class}");
                return Err(Error::type_error(message));
            }
            ("length", Some(View::String(text)), _) => int(code_points(text).count()),
            ("length", Some(View::Array(items)), _) => int(items.len()),
            ("length", Some(View::Object), _) => int(self.entries(first).count()),
            ("ends_with", Some(View::String(text)), Some(View::String(affix))) => {
                rt.boolean(text.ends_with(affix))
            }
            ("starts_with", Some(View::String(text)), Some(View::String(affix))) => {
                rt.boolean(text.starts_with(affix))
            }
            ("reverse", Some(View::String(text)), _) => {
                string(&code_points(text).rev().collect::<Vec<_>>().concat())
            }
            ("reverse", Some(View::Array(items)), _) => {
                rt.array(items.iter().rev().cloned().collect())
            }
            ("join", Some(View::String(separator)), Some(View::Array(items))) => {
                let parts = items.iter().filter_map(|item| match rt.view(item) {
                    View::String(text) => Some(text),
                    _ => None,
                });
                string(&parts.collect::<Vec<_>>().join(separator))
            }
            ("keys", Some(View::Object), _) => {
                rt.array(self.entries(first).map(|(key, _)| string(key)).collect())
            }
            ("values", Some(View::Object), _) => rt.array(
                self.entries(first)
                    .map(|(_, value)| value.clone())
                    .collect(),
            ),
            _ => rt.null(),
        })
    }

    /// Python's `a < b` of two numbers or two strings.
    fn lt(&self, a: &R::Value, b: &R::Value) -> bool {
        self.less("<", a, b).ok().flatten() == Some(Ordering::Less)
    }

    fn numbers(&self, items: &[R::Value]) -> Vec<Number> {
        items
            .iter()
            .filter_map(|item| match self.runtime.view(item) {
                View::Number(number) => Some(number),
                _ => None,
            })
            .collect()
    }

    fn entries<'v>(
        &self,
        object: &'v Val<'_, R::Value>,
    ) -> impl Iterator<Item = (&'v [u8], &'v R::Value)> + 'v
    where
        R::Value: 'v,
    {
        object
            .get()
            .map(|object| self.runtime.entries(object))
            .into_iter()
            .flatten()
    }

    /// `max(items, key=key)` (`op` `>`) or `min` (`<`): the first item
    /// whose key no later one's beats.
    fn best(
        &self,
        items: &[R::Value],
        op: &str,
        key: impl Fn(&R::Value) -> Result<R::Value, Error>,
    ) -> Result<Option<R::Value>, Error> {
        let mut best: Option<(&R::Value, R::Value)> = None;
        for item in items {
            let value = key(item)?;
            let beats = match &best {
                None => true,
                Some((_, current)) => {
                    let order = self.less(op, &value, current)?;
                    order
                        == Some(if op == ">" {
                            Ordering::Greater
                        } else {
                            Ordering::Less
                        })
                }
            };
            if beats {
                best = Some((item, value));
            }
        }
        Ok(best.map(|(item, _)| item.clone()))
    }

    /// `map`, `sort_by`, `min_by` and `max_by`.
    fn higher<'a>(
        &self,
        name: &str,
        first: &Val<'a, R::Value>,
        second: &Val<'a, R::Value>,
    ) -> Eval<'a, R::Value> {
        let rt = self.runtime;
        let (expression, items) = match (first, second) {
            (Val::Expref(expression), array) | (array, Val::Expref(expression)) => {
                (*expression, array)
            }
            _ => return Ok(Val::Own(rt.null())),
        };
        let Some(View::Array(items)) = self.view(items) else {
            return Ok(Val::Own(rt.null()));
        };
        if name == "map" {
            let mapped = items
                .iter()
                .map(|item| self.visit(expression, item)?.into_owned());
            return Ok(Val::Own(rt.array(mapped.collect::<Result<_, _>>()?)));
        }
        self.by(name, items, expression).map(Val::Own)
    }

    /// `sort_by`, `min_by` and `max_by`.
    fn by(
        &self,
        name: &str,
        items: &[R::Value],
        expression: &Node<R::Value>,
    ) -> Result<R::Value, Error> {
        let rt = self.runtime;
        let key_of = |item: &R::Value, allowed: &[&str]| -> Result<R::Value, Error> {
            let key = self.visit(expression, item)?;
            let kind = jmespath_type(self.short_type(&key));
            if !allowed.contains(&kind) {
                return Err(Error::invalid_type(name, &self.str(&key), kind, allowed));
            }
            key.into_owned()
        };
        if name != "sort_by" {
            let op = if name == "max_by" { ">" } else { "<" };
            let best = self.best(items, op, |item| key_of(item, &["number", "string"]))?;
            return Ok(best.unwrap_or_else(|| rt.null()));
        }
        let Some(first) = items.first() else {
            return Ok(rt.array(Vec::new()));
        };
        let kind = jmespath_type(self.short_type(&self.visit(expression, first)?));
        if kind != "number" && kind != "string" {
            return Err(Error::invalid_type(
                name,
                &rt.str(first),
                kind,
                &["string", "number"],
            ));
        }
        let mut keyed = items
            .iter()
            .map(|item| Ok((key_of(item, &[kind])?, item.clone())))
            .collect::<Result<Vec<_>, Error>>()?;
        crate::sort::sort(&mut keyed, |a, b| self.lt(&a.0, &b.0));
        Ok(rt.array(keyed.into_iter().map(|(_, item)| item).collect()))
    }

    /// `merged.update(arg)`.
    fn update(
        &self,
        merged: &mut Vec<(Vec<u8>, R::Value)>,
        arg: &Val<'_, R::Value>,
    ) -> Result<(), Error> {
        let not_iterable = || {
            Error::type_error(format!(
                "'{}' object is not iterable",
                arg.get()
                    .map_or("_Expression", |arg| self.runtime.type_name(arg))
            ))
        };
        match self.view(arg) {
            Some(View::Object) => {
                for (key, value) in self.entries(arg) {
                    match merged.iter_mut().find(|(k, _)| k == key) {
                        Some((_, old)) => *old = value.clone(),
                        None => merged.push((key.to_vec(), value.clone())),
                    }
                }
                Ok(())
            }
            Some(View::String([]) | View::Array([])) => Ok(()),
            Some(View::String(_)) => Err(Error::new(
                "ValueError",
                "dictionary update sequence element #0 has length 1; 2 is required",
            )),
            Some(View::Array(_)) => {
                Err(Error::new("UnsupportedError", "merge() of a list of pairs"))
            }
            Some(View::Other)
                if self
                    .runtime
                    .type_name(arg.get().ok_or_else(Error::expref_value)?)
                    == "bytes" =>
            {
                Err(Error::new("UnsupportedError", "merge() of bytes"))
            }
            _ => Err(not_iterable()),
        }
    }
}

/// The text's code points, each as its bytes in `View::String`'s encoding.
fn code_points(text: &[u8]) -> impl DoubleEndedIterator<Item = &[u8]> {
    let mut starts: Vec<usize> = (0..text.len())
        .filter(|&i| text[i] & 0xC0 != 0x80)
        .collect();
    starts.push(text.len());
    let bounds: Vec<(usize, usize)> = starts.windows(2).map(|pair| (pair[0], pair[1])).collect();
    bounds
        .into_iter()
        .map(move |(start, end)| &text[start..end])
}
