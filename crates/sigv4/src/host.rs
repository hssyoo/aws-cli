//! The `host` header a request to a URL carries.

/// `botocore.auth._host_from_url`: the lowercase host without userinfo, and
/// the port unless it is the scheme's default.
pub fn host_header(scheme: &str, authority: &str) -> String {
    let hostport = authority
        .rsplit_once('@')
        .map_or(authority, |(_, rest)| rest);
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        match rest.split_once(']') {
            Some((inside, tail)) => (format!("[{inside}]"), tail.strip_prefix(':')),
            None => (hostport.to_owned(), None),
        }
    } else {
        match hostport.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port)),
            None => (hostport.to_owned(), None),
        }
    };
    let host = host.to_ascii_lowercase();
    let default = match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    match port.and_then(|port| port.parse::<u32>().ok()) {
        Some(port) if Some(port) != default => format!("{host}:{port}"),
        _ => host,
    }
}
