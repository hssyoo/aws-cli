//! The text of each botocore exception a native command prints, one copy
//! for every path (botocore/exceptions.py, each class's `fmt`, as `str()`
//! gives it). A text that names a value takes it as the caller has it:
//! bytes where the value came off the wire, text otherwise.

/// `NoCredentialsError`.
pub const NO_CREDENTIALS: &str = "Unable to locate credentials";
/// `NoRegionError`.
pub const NO_REGION: &str = "You must specify a region.";

/// `EndpointConnectionError`: refused, or no address for the host.
pub fn endpoint_connection(url: &str) -> String {
    format!("Could not connect to the endpoint URL: \"{url}\"")
}

/// `ConnectTimeoutError`.
pub fn connect_timeout(url: &str) -> String {
    format!("Connect timeout on endpoint URL: \"{url}\"")
}

/// `ReadTimeoutError`.
pub fn read_timeout(url: &str) -> String {
    format!("Read timeout on endpoint URL: \"{url}\"")
}

/// `ConnectionClosedError`.
pub fn connection_closed(url: &str) -> String {
    format!(
        "Connection was closed before we received a valid response from endpoint URL: \
         \"{url}\"."
    )
}

/// `SSLError`: `reason` is the TLS library's, which botocore prints as
/// urllib3 raised it.
pub fn ssl_validation(url: &str, reason: &str) -> String {
    format!("SSL validation failed for {url} {reason}")
}

/// `ProxyConnectionError`: the proxy's URL, credentials masked.
pub fn proxy_connection(proxy_url: &[u8]) -> Vec<u8> {
    let mut text = b"Failed to connect to proxy URL: \"".to_vec();
    text.extend_from_slice(proxy_url);
    text.push(b'"');
    text
}

/// `HTTPClientError`, for anything else the HTTP client raises.
pub fn http_client(reason: &str) -> String {
    format!("An HTTP Client raised an unhandled exception: {reason}")
}

/// `ClientError`: `MSG_TEMPLATE`, with `_get_retry_info`'s note when the
/// attempts ran out after `retries` retries.
pub fn client_error(
    code: &[u8],
    operation: &[u8],
    retries: Option<u64>,
    message: &[u8],
) -> Vec<u8> {
    let mut text = b"An error occurred (".to_vec();
    text.extend_from_slice(code);
    text.extend_from_slice(b") when calling the ");
    text.extend_from_slice(operation);
    text.extend_from_slice(b" operation");
    if let Some(retries) = retries {
        text.extend_from_slice(format!(" (reached max retries: {retries})").as_bytes());
    }
    text.extend_from_slice(b": ");
    text.extend_from_slice(message);
    text
}

#[cfg(test)]
mod tests;
