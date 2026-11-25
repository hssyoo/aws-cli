//! The request and response the call pipeline passes around, as botocore's
//! `AWSPreparedRequest` and its HTTP response hold them.

/// Header names and values in order.
///
/// A request's headers are botocore's `HeadersDict`: setting a name that is
/// there, in any case, replaces its value in place and keeps the name as it
/// was first spelled. A response's keep every header as it arrived, a
/// repeated name repeated ([`Headers::append`]). Lookup ignores case.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Headers(Vec<(String, String)>);

impl Headers {
    pub const fn new() -> Self {
        Self(Vec::new())
    }
    /// Headers in the order given, each appended.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut headers = Self::new();
        for (name, value) in pairs {
            headers.append(name, value);
        }
        headers
    }
    /// The first value of `name`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(other, _)| other.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
    /// `headers[name]` of botocore's `HeadersDict`: every value of `name`,
    /// in any case, joined with `, ` as urllib3 joins a repeated header.
    pub fn joined(&self, name: &str) -> Option<String> {
        let values: Vec<&str> = self.get_all(name).collect();
        (!values.is_empty()).then(|| values.join(", "))
    }
    /// Every value of `name`, in order.
    pub fn get_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.0
            .iter()
            .filter(move |(other, _)| other.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
    /// Sets `name`'s value: in place of the first header of that name, the
    /// others of the name removed, or at the end when there is none.
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self
            .0
            .iter()
            .position(|(other, _)| other.eq_ignore_ascii_case(name))
        {
            Some(first) => {
                self.0[first].1 = value;
                let mut index = 0;
                self.0.retain(|(other, _)| {
                    index += 1;
                    index - 1 == first || !other.eq_ignore_ascii_case(name)
                });
            }
            None => self.0.push((name.to_owned(), value)),
        }
    }
    /// Adds a header at the end, whatever is there.
    pub fn append(&mut self, name: &str, value: impl Into<String>) {
        self.0.push((name.to_owned(), value.into()));
    }
    /// Removes every header named `name`.
    pub fn remove(&mut self, name: &str) {
        self.0
            .retain(|(other, _)| !other.eq_ignore_ascii_case(name));
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Names the headers only: a value may be a token or a signature.
impl std::fmt::Debug for Headers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(name, _)| name))
            .finish()
    }
}

/// A request method, as a model's `http.method` spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Patch,
    Options,
}

impl Method {
    /// The method `text` names, exactly as spelled; `None` for another.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "GET" => Self::Get,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "DELETE" => Self::Delete,
            "HEAD" => Self::Head,
            "PATCH" => Self::Patch,
            "OPTIONS" => Self::Options,
            _ => return None,
        })
    }
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Patch => "PATCH",
            Self::Options => "OPTIONS",
        }
    }
}

/// A request URL in the four parts the pipeline joins and signs. Endpoints
/// come from rule sets or the user, already encoded; nothing here resolves
/// or normalizes them.
#[derive(Clone, PartialEq, Eq)]
pub struct Url {
    pub scheme: String,
    /// `host[:port]`, as written, an IPv6 host bracketed.
    pub authority: String,
    /// Starts with `/`; percent-encoded.
    pub path: String,
    /// Without the `?`; encoded.
    pub query: String,
}

impl Url {
    /// `scheme://authority[/path][?query]`; `None` without a scheme or an
    /// authority. A fragment is not split off: endpoints carry none.
    pub fn parse(text: &str) -> Option<Self> {
        let (scheme, rest) = text.split_once("://")?;
        let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        if scheme.is_empty() || authority.is_empty() {
            return None;
        }
        Some(Self {
            scheme: scheme.to_owned(),
            authority: authority.to_owned(),
            path: if path.is_empty() { "/" } else { path }.to_owned(),
            query: query.to_owned(),
        })
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}{}", self.scheme, self.authority, self.path)?;
        if !self.query.is_empty() {
            write!(f, "?{}", self.query)?;
        }
        Ok(())
    }
}

/// Leaves out the query, which may hold a presigned signature or a token.
impl std::fmt::Debug for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Url")
            .field("scheme", &self.scheme)
            .field("authority", &self.authority)
            .field("path", &self.path)
            .field("query_len", &self.query.len())
            .finish()
    }
}

/// A request as it is sent.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: Url,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// Gives the body's length: a body may hold credentials.
impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("headers", &self.headers)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// A response, read whole.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// Gives the body's length: a body may hold credentials.
impl std::fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body_len", &self.body.len())
            .finish()
    }
}

#[cfg(test)]
mod tests;
