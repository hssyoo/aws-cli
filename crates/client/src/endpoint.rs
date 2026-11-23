//! Endpoint rule parameters by name, as `EndpointRulesetResolver` gathers
//! them for one call (`_get_provider_params`), which the `endpoint_params`
//! stage may change before the rules engine reads them.

/// A rule parameter's value, of a kind a rule set declares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamValue {
    String(String),
    Boolean(bool),
    StringArray(Vec<String>),
}

/// The parameters of one resolution, in the order they were set; a name
/// left out is unset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EndpointParams(Vec<(String, ParamValue)>);

impl EndpointParams {
    /// Sets `name`, in place when it is set already.
    pub fn set(&mut self, name: &str, value: ParamValue) {
        match self.0.iter_mut().find(|(other, _)| other == name) {
            Some((_, old)) => *old = value,
            None => self.0.push((name.to_owned(), value)),
        }
    }
    pub fn get(&self, name: &str) -> Option<&ParamValue> {
        self.0
            .iter()
            .find(|(other, _)| other == name)
            .map(|(_, value)| value)
    }
    /// Unsets `name`.
    pub fn remove(&mut self, name: &str) {
        self.0.retain(|(other, _)| other != name);
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ParamValue)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }
}
