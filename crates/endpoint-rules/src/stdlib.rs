//! The rules language's standard library 1.0, as the CRT engine
//! implements it (`endpoints_standard_lib.c`, with `aws-c-common`'s URI
//! parser and host checks): each function takes its arguments' exact
//! types, and an unset or mistyped argument fails evaluation.
use crate::error::EvalError;
use crate::ir::{EvalValue, FnId};
use std::sync::Arc;

type Arg = Option<EvalValue>;

fn wrong(name: &str) -> EvalError {
    EvalError::new(format!("{name}: an argument is unset or of the wrong type"))
}

fn string<'a>(arg: &'a Arg, name: &str) -> Result<&'a Arc<str>, EvalError> {
    match arg {
        Some(EvalValue::Str(text)) => Ok(text),
        _ => Err(wrong(name)),
    }
}

fn boolean(arg: &Arg, name: &str) -> Result<bool, EvalError> {
    match arg {
        Some(EvalValue::Bool(flag)) => Ok(*flag),
        _ => Err(wrong(name)),
    }
}

fn int(arg: &Arg, name: &str) -> Result<i64, EvalError> {
    match arg {
        Some(EvalValue::Int(number)) => Ok(*number),
        _ => Err(wrong(name)),
    }
}

/// Calls `id` on its evaluated arguments; `None` is an unset result.
pub(crate) fn call(id: FnId, args: &[Arg]) -> Result<Option<EvalValue>, EvalError> {
    let bool_of = |flag| Ok(Some(EvalValue::Bool(flag)));
    match (id, args) {
        (FnId::IsSet, [value]) => bool_of(value.is_some()),
        (FnId::Not, [value]) => bool_of(!boolean(value, "not")?),
        (FnId::BooleanEquals, [a, b]) => {
            bool_of(boolean(a, "booleanEquals")? == boolean(b, "booleanEquals")?)
        }
        (FnId::StringEquals, [a, b]) => {
            bool_of(string(a, "stringEquals")? == string(b, "stringEquals")?)
        }
        (FnId::IsValidHostLabel, [label, subdomains]) => bool_of(is_valid_host_label(
            string(label, "isValidHostLabel")?,
            boolean(subdomains, "isValidHostLabel")?,
        )),
        (FnId::ParseUrl, [url]) => Ok(parse_url(string(url, "parseURL")?)),
        (FnId::UriEncode, [text]) => Ok(Some(EvalValue::Str(
            uri_encode(string(text, "uriEncode")?).into(),
        ))),
        (FnId::Substring, [text, start, stop, reverse]) => Ok(substring(
            string(text, "substring")?,
            int(start, "substring")?,
            int(stop, "substring")?,
            boolean(reverse, "substring")?,
        )),
        _ => Err(EvalError::new(format!("{id:?}: wrong number of arguments"))),
    }
}

/// `aws_is_valid_host_label`: labels of 1 to 63 letters, digits and `-`,
/// each starting and ending with a letter or digit; with `subdomains`,
/// several such labels joined by `.`.
pub(crate) fn is_valid_host_label(label: &str, subdomains: bool) -> bool {
    let bytes = label.as_bytes();
    let Some(&last) = bytes.last() else {
        return false;
    };
    let mut start_of_label = true;
    let mut length = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'.' {
            if !subdomains || length == 0 || !bytes[i - 1].is_ascii_alphanumeric() {
                return false;
            }
            start_of_label = true;
            length = 0;
            continue;
        }
        if !byte.is_ascii_alphanumeric() && (start_of_label || byte != b'-') {
            return false;
        }
        start_of_label = false;
        length += 1;
        if length > 63 {
            return false;
        }
    }
    last.is_ascii_alphanumeric()
}

/// `substring(input, start, stop, reverse)`: unset unless
/// `0 <= start < stop <= len` and `input` is ASCII. (The CRT engine casts a
/// negative `start` to an index; here it is unset.)
fn substring(text: &str, start: i64, stop: i64, reverse: bool) -> Option<EvalValue> {
    let len = i64::try_from(text.len()).ok()?;
    if start >= stop || len < stop || start < 0 || !text.is_ascii() {
        return None;
    }
    let (from, to) = if reverse {
        (len - stop, len - start)
    } else {
        (start, stop)
    };
    let piece = text.get(usize::try_from(from).ok()?..usize::try_from(to).ok()?)?;
    Some(EvalValue::str(piece))
}

/// `aws_byte_buf_append_encoding_uri_param`: every byte but letters,
/// digits and `-_.~` as `%XX`.
fn uri_encode(text: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    out
}

/// A URL split as `aws_uri_init_parse` splits it.
struct Uri<'a> {
    scheme: &'a str,
    authority: &'a str,
    host: &'a [u8],
    path: &'a str,
    query: &'a str,
}

/// `parseURL`: unset unless `url` parses, has no query, an `http` or
/// `https` scheme and an authority.
fn parse_url(url: &str) -> Option<EvalValue> {
    let uri = parse_uri(url)?;
    if !uri.query.is_empty() || !matches!(uri.scheme, "http" | "https") || uri.authority.is_empty()
    {
        return None;
    }
    let mut normalized = String::with_capacity(uri.path.len() + 2);
    if !uri.path.starts_with('/') {
        normalized.push('/');
    }
    normalized.push_str(uri.path);
    if !normalized.ends_with('/') {
        normalized.push('/');
    }
    let is_ip = is_ipv4(uri.host) || is_ipv6(uri.host);
    let fields: [(&str, EvalValue); 5] = [
        ("scheme", EvalValue::str(uri.scheme)),
        ("authority", EvalValue::str(uri.authority)),
        ("path", EvalValue::str(uri.path)),
        ("normalizedPath", EvalValue::Str(normalized.into())),
        ("isIp", EvalValue::Bool(is_ip)),
    ];
    Some(EvalValue::Record(fields.map(|(k, v)| (k.into(), v)).into()))
}

/// `aws_uri_init_parse`'s state machine: the scheme ends at the first
/// `:` if a `/` follows it, the authority at the first `/` (or, failing
/// one, `?`), the path at the next `?`.
fn parse_uri(url: &str) -> Option<Uri<'_>> {
    let mut rest = url;
    let mut scheme = "";
    if let Some(colon) = url.find(':')
        && url.as_bytes().get(colon + 1) == Some(&b'/')
    {
        scheme = &url[..colon];
        rest = url[colon..].strip_prefix("://")?;
    }
    if rest.is_empty() {
        return None;
    }
    let end = rest
        .find('/')
        .or_else(|| rest.find('?'))
        .unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(end);
    let host = host(authority)?;
    let (path, query) = match rest.strip_prefix('?') {
        Some(query) => ("", query),
        None => rest.split_once('?').unwrap_or((rest, "")),
    };
    Some(Uri {
        scheme,
        authority,
        host,
        path,
        query,
    })
}

/// The host of an authority `[userinfo@]host[:port]`, brackets dropped
/// from an IPv6 literal; `None` if the port is not a `u32` or a `[` is
/// not closed.
fn host(authority: &str) -> Option<&[u8]> {
    let hostport = authority
        .split_once('@')
        .map_or(authority, |(_, rest)| rest);
    let bytes = hostport.as_bytes();
    let (host, port) = if hostport.starts_with('[') {
        let close = hostport.find(']')?;
        match hostport[close..].find(':') {
            Some(colon) => (&bytes[1..close + colon - 1], &hostport[close + colon + 1..]),
            None => (&bytes[1..bytes.len() - 1], ""),
        }
    } else {
        let (host, port) = hostport.split_once(':').unwrap_or((hostport, ""));
        (host.as_bytes(), port)
    };
    let port_ok = port.is_empty()
        || (port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u32>().is_ok());
    port_ok.then_some(host)
}

/// `aws_host_utils_is_ipv4`: at most 15 bytes read by
/// `sscanf("%03hu.%03hu.%03hu.%03hu%1s")`, each octet at most 255.
pub(crate) fn is_ipv4(host: &[u8]) -> bool {
    if host.len() > 15 {
        return false;
    }
    // `sscanf` reads a C string: an embedded NUL ends it.
    let text = host.split(|&b| b == 0).next().unwrap_or_default();
    let mut at = 0;
    for octet in 0..4 {
        if octet > 0 {
            if text.get(at) != Some(&b'.') {
                return false;
            }
            at += 1;
        }
        match scan_octet(text, at) {
            Some((value, next)) if value <= 255 => at = next,
            _ => return false,
        }
    }
    text[at..].iter().all(|b| is_c_space(*b))
}

fn is_c_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// One `%3hu`: leading white space, then up to three bytes of an optional
/// sign and at least one digit, as an unsigned short.
fn scan_octet(text: &[u8], mut at: usize) -> Option<(u16, usize)> {
    while text.get(at).copied().is_some_and(is_c_space) {
        at += 1;
    }
    let end = (at + 3).min(text.len());
    let negative = matches!(text.get(at), Some(b'-'));
    let first_digit = at + usize::from(matches!(text.get(at), Some(b'-' | b'+')));
    let digits = text.get(first_digit..end)?;
    let count = digits.iter().take_while(|b| b.is_ascii_digit()).count();
    if count == 0 {
        return None;
    }
    let magnitude = digits[..count]
        .iter()
        .fold(0u16, |n, d| n * 10 + u16::from(d - b'0'));
    let value = if negative {
        magnitude.wrapping_neg()
    } else {
        magnitude
    };
    Some((value, first_digit + count))
}

/// `aws_host_utils_is_ipv6(host, true)`: up to eight groups of at most
/// four hex digits, one `::` at most, and a zone `%25…` of letters and
/// digits.
pub(crate) fn is_ipv6(host: &[u8]) -> bool {
    let mut pieces = host.split(|&b| b == b'%');
    let address = pieces.next().unwrap_or_default();
    let len = address.len();
    if host.is_empty()
        || !(2..=39).contains(&len)
        || !address.iter().all(|b| b.is_ascii_hexdigit() || *b == b':')
        || (address[0] == b':' && address[1] != b':')
        || (address[len - 1] == b':' && address[len - 2] != b':')
    {
        return false;
    }
    let Some((groups, double_colon)) = ipv6_groups(address) else {
        return false;
    };
    if let Some(zone) = pieces.next()
        && (zone.len() < 3
            || !zone.starts_with(b"25")
            || !zone.iter().all(u8::is_ascii_alphanumeric))
    {
        return false;
    }
    if double_colon {
        groups <= 8
    } else {
        groups == 8
    }
}

/// The group count and whether a `::` appears, or `None` past eight
/// groups, four digits in a group or a second `::`.
fn ipv6_groups(address: &[u8]) -> Option<(u8, bool)> {
    let (mut groups, mut digits, mut double_colon) = (1u8, 0u8, false);
    for (i, &byte) in address.iter().enumerate() {
        if byte == b':' {
            groups += 1;
            digits = 0;
            if i > 0 && address[i - 1] == b':' {
                if double_colon {
                    return None;
                }
                double_colon = true;
                groups -= 1;
            }
        } else {
            digits += 1;
        }
        if digits > 4 || groups > 8 {
            return None;
        }
    }
    Some((groups, double_colon))
}
