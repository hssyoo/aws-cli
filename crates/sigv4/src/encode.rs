//! URI and query encoding as botocore's `quote` and canonical query do.

/// Python's `urllib.parse.quote(text, safe=safe)`: ASCII letters, digits and
/// `_.-~` stay, as do the characters of `safe`; every other byte is `%XX`.
pub fn quote(text: &str, safe: &str) -> String {
    quote_bytes(text.as_bytes(), safe)
}

/// [`quote`] of bytes that need not be UTF-8, as Python quotes `bytes`.
pub fn quote_bytes(bytes: &[u8], safe: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut quoted = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric()
            || b"_.-~".contains(&byte)
            || safe.as_bytes().contains(&byte)
        {
            quoted.push(char::from(byte));
        } else {
            quoted.push('%');
            quoted.push(char::from(HEX[usize::from(byte >> 4)]));
            quoted.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    quoted
}

/// `SigV4Auth._canonical_query_string_url`: the pairs of an encoded query,
/// split at the first `=`, sorted, and joined again.
pub(crate) fn canonical_query(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    let mut pairs: Vec<(&str, &str)> = query
        .split('&')
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
        .collect();
    pairs.sort_unstable();
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// `SigV4Auth._normalize_url_path`: `botocore.utils.normalize_url_path`
/// quoted with `/~` safe.
pub(crate) fn normalize_path(path: &str) -> String {
    quote(&normalize_url_path(path), "/~")
}

/// `botocore.utils.normalize_url_path`: `/` for an empty path, else
/// [`remove_dot_segments`].
fn normalize_url_path(path: &str) -> String {
    if path.is_empty() {
        return "/".to_owned();
    }
    remove_dot_segments(path)
}

/// `botocore.utils.remove_dot_segments`: RFC 3986's dot segments removed
/// (a `..` past the start drops nothing) and, as AWS services require, the
/// empty segments too, so `//` collapses. A leading `/` stays; a trailing
/// one stays when the path ends in `/` and a segment is left.
fn remove_dot_segments(url: &str) -> String {
    if url.is_empty() {
        return String::new();
    }
    let mut output: Vec<&str> = Vec::new();
    for segment in url.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                output.pop();
            }
            _ => output.push(segment),
        }
    }
    let first = if url.starts_with('/') { "/" } else { "" };
    let last = if url.ends_with('/') && !output.is_empty() {
        "/"
    } else {
        ""
    };
    format!("{first}{}{last}", output.join("/"))
}
