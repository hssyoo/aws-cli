//! The seam between the evaluator and an object model: how a value reads
//! as JSON's types, how one is built, and the Python behaviour the library
//! borrows from the interpreter it runs in (`json.loads`, `str()`,
//! `json.dumps`, `int()`).
use crate::error::Error;
use crate::number::Number;

/// A value as JMESPath's types see it.
#[derive(Debug)]
pub enum View<'a, V> {
    Null,
    Bool(bool),
    Number(Number),
    /// A `str`, in `pytext::wtf8`'s encoding: UTF-8 with each lone
    /// surrogate encoded on its own, so the bytes compare as the code
    /// points do.
    String(&'a [u8]),
    Array(&'a [V]),
    /// A `dict`, read through [`Runtime::get`] and [`Runtime::entries`].
    Object,
    /// Any other Python object (`bytes`, `datetime`): JMESPath's
    /// `unknown` type, of which [`Runtime::type_name`] gives the class.
    Other,
}

/// An object model the evaluator reads and builds, and the Python it
/// stands in for.
pub trait Runtime {
    type Value: Clone;

    fn view<'a>(&self, value: &'a Self::Value) -> View<'a, Self::Value>;
    /// `object.get(key)` of a dict; `None` for anything else.
    fn get<'a>(&self, object: &'a Self::Value, key: &[u8]) -> Option<&'a Self::Value>;
    /// A dict's items in its order; nothing for anything else.
    fn entries<'a>(
        &self,
        object: &'a Self::Value,
    ) -> impl Iterator<Item = (&'a [u8], &'a Self::Value)> + 'a;

    fn null(&self) -> Self::Value;
    fn boolean(&self, value: bool) -> Self::Value;
    fn number(&self, value: Number) -> Self::Value;
    /// A `str` of text in [`View::String`]'s encoding.
    fn string(&self, text: Vec<u8>) -> Self::Value;
    fn array(&self, items: Vec<Self::Value>) -> Self::Value;
    /// A dict of `entries`, in order; a repeated key keeps its first place
    /// and takes its last value, as `dict[key] = value` does.
    fn object(&self, entries: Vec<(Vec<u8>, Self::Value)>) -> Self::Value;

    /// `json.loads(text)`; `Err` holds the `ValueError`'s text.
    ///
    /// # Errors
    /// Text `json.loads` refuses.
    fn loads(&self, text: &str) -> Result<Self::Value, String>;
    /// The class's name as Python's messages print it (`tp_name`):
    /// `NoneType`, `bool`, `int`, `float`, `str`, `list`, `dict`, `bytes`,
    /// `datetime.datetime`.
    fn type_name(&self, value: &Self::Value) -> &'static str;
    /// `str(value)`, for a message.
    fn str(&self, value: &Self::Value) -> String;
    /// `json.dumps(value, separators=(',', ':'), default=str)`.
    fn dumps(&self, value: &Self::Value) -> String;
    /// `a == b` of two [`View::Other`] values.
    fn other_eq(&self, a: &Self::Value, b: &Self::Value) -> bool;
    /// `bool(value)` of a [`View::Other`] value.
    fn other_truthy(&self, value: &Self::Value) -> bool;
    /// `int(value)`, else `float(value)`, of a `str` or a
    /// [`View::Other`] value: `None` when both raise `ValueError`.
    ///
    /// # Errors
    /// Any other exception, such as `int()`'s `TypeError` for a type it
    /// does not convert.
    fn to_number(&self, value: &Self::Value) -> Result<Option<Number>, Error>;
}
