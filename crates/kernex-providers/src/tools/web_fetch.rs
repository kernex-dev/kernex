//! The `web_fetch` tool and the policy that gates it.
//!
//! A fetch is an outbound channel: text injected into a session can ask the
//! model to read something and send it away in a query string. The tool is
//! therefore off unless the host turns it on, and the host decides which
//! hosts it may reach. Every hop of a redirect chain is checked like the
//! first URL.

use serde_json::Value;
use tracing::debug;

use super::{truncate_output, ToolExecutor, ToolResult, MAX_READ_OUTPUT};

/// Redirects followed before a fetch is abandoned.
const MAX_REDIRECTS: usize = 5;

/// Upper bound for one call, redirects and name lookups included.
const FETCH_TIMEOUT_SECS: u64 = 60;

/// Which hosts the `web_fetch` tool may reach.
///
/// The policy governs the runtime's own `web_fetch` tool. A provider that
/// is itself an agent CLI brings its own fetch and search tools, which
/// cannot be held to a host list: they run only under `AllowAll`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebFetchPolicy {
    /// The tool is not offered to the model and refuses every call.
    #[default]
    Disabled,
    /// Only these hosts. An entry is a host name matched exactly, or
    /// `*.example.com` for every subdomain of `example.com` (not
    /// `example.com` itself). Matching ignores case. An IP address is
    /// matched as written.
    AllowHosts(Vec<String>),
    /// Any public host.
    AllowAll,
}

impl WebFetchPolicy {
    /// Whether the tool is offered at all.
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// Whether a request to `host` is allowed.
    pub fn permits(&self, host: &str) -> bool {
        match self {
            Self::Disabled => false,
            Self::AllowAll => true,
            Self::AllowHosts(entries) => {
                let host = normalize_host(host);
                entries.iter().any(|entry| {
                    let entry = normalize_host(entry);
                    match entry.strip_prefix("*.") {
                        Some(parent) => host
                            .strip_suffix(parent)
                            .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.')),
                        None => host == entry,
                    }
                })
            }
        }
    }
}

/// Lower-case a host and drop the brackets of an IPv6 literal and the dot
/// that ends a fully qualified name.
fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn error(content: impl Into<String>) -> ToolResult {
    ToolResult {
        content: content.into(),
        is_error: true,
    }
}

/// What one request of a fetch produced.
enum Hop {
    /// The final answer for the model.
    Done(ToolResult),
    /// The server pointed somewhere else.
    Redirect(String),
}

impl ToolExecutor {
    /// Whether loopback and plain `http` targets are accepted. Only a test
    /// can turn this on, to fetch from a server it started itself.
    fn fetch_allows_local(&self) -> bool {
        #[cfg(test)]
        {
            self.fetch_allows_local
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    /// Fetch a URL and return its text content.
    pub(super) async fn exec_web_fetch(&self, args: &Value) -> ToolResult {
        let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
        if url.is_empty() {
            return error("Error: 'url' parameter is required");
        }
        if !self.web_fetch.is_enabled() {
            return error("Error: web_fetch is disabled by the host");
        }

        debug!("tool/web_fetch: {url}");

        let limit = std::time::Duration::from_secs(FETCH_TIMEOUT_SECS);
        match tokio::time::timeout(limit, self.fetch_following_redirects(url)).await {
            Ok(result) => result,
            Err(_) => error(format!(
                "Error: fetch of '{url}' timed out after {FETCH_TIMEOUT_SECS}s"
            )),
        }
    }

    async fn fetch_following_redirects(&self, url: &str) -> ToolResult {
        let mut current = url.to_string();
        // What an error names. A redirect target is the server's text, so
        // only its scheme and host are repeated.
        let mut label = format!("'{url}'");
        for _ in 0..=MAX_REDIRECTS {
            match self.fetch_hop(&current, &label).await {
                Hop::Done(result) => return result,
                Hop::Redirect(next) => {
                    label = redirect_label(&next);
                    current = next;
                }
            }
        }
        error(format!(
            "Error: refusing to fetch '{url}': more than {MAX_REDIRECTS} redirects"
        ))
    }

    /// Send one request. The URL is validated, checked against the policy
    /// and pinned to the addresses that were validated, whether it came from
    /// the model or from a `Location` header.
    async fn fetch_hop(&self, url: &str, label: &str) -> Hop {
        let local = self.fetch_allows_local();
        // SSRF guard: only allow https://, reject loopback / private / link-local
        // / cloud-metadata IP ranges so a tool-using model cannot probe
        // 169.254.169.254, 127.0.0.1:11434 (other agents on the host),
        // 10.x / 192.168.x / fd00::/8 networks. file://, ftp://, gopher://
        // and other schemes are blocked by the https check.
        let parsed = if local {
            match reqwest::Url::parse(url) {
                Ok(parsed) => parsed,
                Err(_) => return Hop::Done(refused(label, "not a valid URL")),
            }
        } else {
            match is_safe_fetch_url(url) {
                Ok(parsed) => parsed,
                Err(reason) => return Hop::Done(refused(label, reason)),
            }
        };
        // Credentials in a URL would be sent as an Authorization header.
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Hop::Done(refused(label, "the URL carries credentials"));
        }
        // The name that is validated and pinned must be the name the client
        // looks up, and a trailing dot makes them differ.
        if parsed.host_str().is_some_and(|h| h.ends_with('.')) {
            return Hop::Done(refused(label, "the host ends with a dot"));
        }
        let host = normalize_host(parsed.host_str().unwrap_or(""));
        if !self.web_fetch.permits(&host) {
            return Hop::Done(refused(
                label,
                "the host is not allowed by the web_fetch policy",
            ));
        }

        let mut builder = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none());

        // Resolve-time SSRF pinning: a hostname that passes the syntactic
        // check can still resolve to a private address (attacker-controlled
        // DNS, rebinding). Resolve it here, validate EVERY address, then pin
        // the connection to the validated set so the answer cannot change
        // between check and connect. Literal-IP hosts were already validated
        // above.
        if host.parse::<std::net::IpAddr>().is_err() {
            let port = parsed.port_or_known_default().unwrap_or(443);
            let addrs: Vec<std::net::SocketAddr> =
                match tokio::net::lookup_host((host.as_str(), port)).await {
                    Ok(it) => it.collect(),
                    Err(e) => {
                        return Hop::Done(error(format!("Error: could not resolve '{host}': {e}")))
                    }
                };
            if addrs.is_empty() {
                return Hop::Done(error(format!("Error: '{host}' resolved to no addresses")));
            }
            if !local {
                if let Some(bad) = addrs.iter().find(|a| !is_public_ip(&a.ip())) {
                    return Hop::Done(error(format!(
                        "Error: refusing to fetch {label}: host resolves to a non-public address ({})",
                        bad.ip()
                    )));
                }
            }
            builder = builder.resolve_to_addrs(&host, &addrs);
        }

        let client = match builder.build() {
            Ok(c) => c,
            Err(e) => return Hop::Done(error(format!("Failed to build HTTP client: {e}"))),
        };
        let mut resp = match client.get(parsed.clone()).send().await {
            Ok(resp) => resp,
            Err(e) => return Hop::Done(error(format!("HTTP request failed: {e}"))),
        };

        let status = resp.status();
        if status.is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok());
            return match location.and_then(|target| parsed.join(target).ok()) {
                Some(next) => Hop::Redirect(next.to_string()),
                None => Hop::Done(error(format!(
                    "Error: {label} answered {status} without a usable Location"
                ))),
            };
        }

        // Read no more than the model will be shown: the body is not ours
        // and may be endless.
        let mut body: Vec<u8> = Vec::new();
        while body.len() <= MAX_READ_OUTPUT {
            match resp.chunk().await {
                Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                Ok(None) => break,
                Err(e) => return Hop::Done(error(format!("Failed to read response body: {e}"))),
            }
        }
        Hop::Done(ToolResult {
            content: truncate_output(&String::from_utf8_lossy(&body), MAX_READ_OUTPUT),
            is_error: !status.is_success(),
        })
    }
}

fn refused(label: &str, reason: &str) -> ToolResult {
    error(format!("Error: refusing to fetch {label}: {reason}"))
}

/// How an error names a redirect target: scheme and host, nothing else.
fn redirect_label(target: &str) -> String {
    match reqwest::Url::parse(target) {
        Ok(url) => format!(
            "a redirect to {}://{}",
            url.scheme(),
            url.host_str().unwrap_or("")
        ),
        Err(_) => "a redirect".to_string(),
    }
}

/// Validate a URL for `web_fetch` against SSRF abuse.
///
/// Returns the parsed URL only when it is `https://` and its host is not a
/// literal loopback, private, link-local, unspecified or cloud-metadata
/// address. A host name is accepted here and checked again after it is
/// resolved.
pub(super) fn is_safe_fetch_url(url: &str) -> Result<reqwest::Url, &'static str> {
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return Err("not a valid URL"),
    };
    if parsed.scheme() != "https" {
        return Err("only https:// is allowed");
    }
    let host = parsed.host_str().ok_or("missing host")?;
    // For IPv6, host_str() returns the address without surrounding brackets in
    // recent reqwest, but older versions wrapped it. Strip just in case.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if !is_public_ip(&ip) {
            return Err("host is a loopback / private / link-local / metadata IP");
        }
    }
    Ok(parsed)
}

fn is_public_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                // 0.0.0.0/8 (this host), 240.0.0.0/4 (reserved),
                // 198.18.0.0/15 (benchmarking), 192.0.0.0/24 (protocol use).
                || octets[0] == 0
                || octets[0] >= 240
                || (octets[0] == 198 && (octets[1] & 0xfe) == 18)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                // 169.254.169.254 — AWS / GCP / Azure instance metadata.
                || (octets[0] == 169 && octets[1] == 254)
                // 100.64.0.0/10 — carrier-grade NAT, often points at internal services.
                || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 64))
        }
        std::net::IpAddr::V6(v6) => {
            // An IPv4 address written as IPv6 is judged as that IPv4 address.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(&std::net::IpAddr::V4(v4));
            }
            let seg = v6.segments();
            !(v6.is_loopback()
                // ::/96 (IPv4-compatible), 64:ff9b::/96 (NAT64) and 2002::/16
                // (6to4) all carry an IPv4 address that is not checked here.
                || seg[..6] == [0; 6]
                || (seg[0] == 0x64 && seg[1] == 0xff9b)
                || seg[0] == 0x2002
                // fec0::/10 — site-local.
                || (seg[0] & 0xffc0) == 0xfec0
                || v6.is_unspecified()
                || v6.is_multicast()
                // fd00::/8 — unique local addresses.
                || (v6.octets()[0] & 0xfe) == 0xfc
                // fe80::/10 — link-local.
                || (v6.octets()[0] == 0xfe && (v6.octets()[1] & 0xc0) == 0x80))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn test_policy_is_disabled_by_default() {
        let policy = WebFetchPolicy::default();
        assert!(!policy.is_enabled());
        assert!(!policy.permits("example.com"));
        assert!(WebFetchPolicy::AllowAll.permits("example.com"));
    }

    #[test]
    fn test_addresses_that_only_look_public_are_rejected() {
        for url in [
            "https://[::ffff:127.0.0.1]/",
            "https://[::ffff:169.254.169.254]/",
            "https://[::ffff:10.0.0.1]/",
            "https://[::127.0.0.1]/",
            "https://[64:ff9b::7f00:1]/",
            "https://[2002:7f00:1::]/",
            "https://[fec0::1]/",
            "https://0.1.2.3/",
            "https://240.0.0.1/",
            "https://198.18.0.1/",
            "https://192.0.0.1/",
        ] {
            assert!(is_safe_fetch_url(url).is_err(), "{url}");
        }
        for url in [
            "https://93.184.216.34/",
            "https://[2606:4700::1111]/",
            "https://198.20.0.1/",
        ] {
            assert!(is_safe_fetch_url(url).is_ok(), "{url}");
        }
    }

    #[tokio::test]
    async fn test_credentials_and_trailing_dot_are_refused() {
        let port = serve().await;
        let tools = executor(WebFetchPolicy::AllowAll);
        let creds = fetch(&tools, format!("http://user:secret@127.0.0.1:{port}/ok")).await;
        assert!(creds.is_error && creds.content.contains("credentials"));
        let dotted = fetch(&tools, format!("http://localhost.:{port}/ok")).await;
        assert!(dotted.is_error && dotted.content.contains("ends with a dot"));
        // The same rules apply to a redirect target.
        let hop = fetch(
            &tools,
            format!("http://127.0.0.1:{port}/to/http://u:p@127.0.0.1:{port}/ok"),
        )
        .await;
        assert!(hop.is_error && hop.content.contains("credentials"));
        assert!(!hop.content.contains("u:p"), "{}", hop.content);
    }

    #[tokio::test]
    async fn test_refused_redirect_names_only_scheme_and_host() {
        let port = serve().await;
        let tools = executor(WebFetchPolicy::AllowHosts(vec!["127.0.0.1".to_string()]));
        let result = fetch(
            &tools,
            format!("http://127.0.0.1:{port}/to/http://localhost:{port}/x?note=SYSTEM-says-ok"),
        )
        .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("a redirect to http://localhost"),
            "{}",
            result.content
        );
        assert!(
            !result.content.contains("SYSTEM-says-ok"),
            "{}",
            result.content
        );
    }

    #[test]
    fn test_allow_hosts_matches_exactly_or_by_subdomain() {
        let policy = WebFetchPolicy::AllowHosts(vec![
            "docs.rs".to_string(),
            "*.example.com".to_string(),
            "203.0.113.7".to_string(),
        ]);
        for host in [
            "docs.rs",
            "DOCS.RS",
            "docs.rs.",
            "api.example.com",
            "a.b.example.com",
        ] {
            assert!(policy.permits(host), "{host}");
        }
        for host in [
            "evil-docs.rs",
            "docs.rs.evil.test",
            "example.com",
            "badexample.com",
            "example.com.evil.test",
            "203.0.113.8",
            "",
        ] {
            assert!(!policy.permits(host), "{host}");
        }
    }

    /// A local HTTP server. `/ok` answers 200, `/to/<url>` redirects to
    /// `<url>`, `/loop` redirects to itself, `/big` sends a body far over
    /// the output cap.
    async fn serve() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let response = if let Some(target) = path.strip_prefix("/to/") {
                        format!(
                            "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                    } else if path == "/loop" {
                        "HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    } else if path == "/big" {
                        let body = "x".repeat(MAX_READ_OUTPUT * 4);
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    } else {
                        "HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nfetched-body"
                            .to_string()
                    };
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        port
    }

    fn executor(policy: WebFetchPolicy) -> ToolExecutor {
        let mut tools = ToolExecutor::new(PathBuf::from("/tmp")).with_web_fetch_policy(policy);
        tools.fetch_allows_local = true;
        tools
    }

    async fn fetch(tools: &ToolExecutor, url: String) -> ToolResult {
        tools.exec_web_fetch(&json!({ "url": url })).await
    }

    #[tokio::test]
    async fn test_disabled_policy_refuses_and_hides_the_tool() {
        let port = serve().await;
        let tools = executor(WebFetchPolicy::Disabled);
        let result = fetch(&tools, format!("http://127.0.0.1:{port}/ok")).await;
        assert!(result.is_error && result.content.contains("disabled"));
        assert!(!tools.all_tool_defs().iter().any(|d| d.name == "web_fetch"));

        let enabled = executor(WebFetchPolicy::AllowAll);
        assert!(enabled
            .all_tool_defs()
            .iter()
            .any(|d| d.name == "web_fetch"));
    }

    #[tokio::test]
    async fn test_host_outside_the_policy_is_refused() {
        let port = serve().await;
        let tools = executor(WebFetchPolicy::AllowHosts(vec!["127.0.0.1".to_string()]));
        let allowed = fetch(&tools, format!("http://127.0.0.1:{port}/ok")).await;
        assert!(!allowed.is_error, "{}", allowed.content);
        assert_eq!(allowed.content, "fetched-body");

        // The same server under a name the policy does not list.
        let other = fetch(&tools, format!("http://localhost:{port}/ok")).await;
        assert!(other.is_error && other.content.contains("not allowed"));
        assert!(!other.content.contains("fetched-body"));
    }

    #[tokio::test]
    async fn test_redirect_to_a_host_outside_the_policy_is_refused() {
        let port = serve().await;
        let tools = executor(WebFetchPolicy::AllowHosts(vec!["127.0.0.1".to_string()]));

        // A permitted host that redirects to a permitted host is followed.
        let inside = fetch(
            &tools,
            format!("http://127.0.0.1:{port}/to/http://127.0.0.1:{port}/ok"),
        )
        .await;
        assert_eq!(inside.content, "fetched-body");

        // A permitted host that redirects to another host is not.
        let outside = fetch(
            &tools,
            format!("http://127.0.0.1:{port}/to/http://localhost:{port}/ok"),
        )
        .await;
        assert!(outside.is_error && outside.content.contains("not allowed"));
        assert!(!outside.content.contains("fetched-body"));
    }

    #[tokio::test]
    async fn test_redirect_loop_and_large_body_are_bounded() {
        let port = serve().await;
        let tools = executor(WebFetchPolicy::AllowAll);
        let looped = fetch(&tools, format!("http://127.0.0.1:{port}/loop")).await;
        assert!(looped.is_error && looped.content.contains("redirects"));

        let big = fetch(&tools, format!("http://127.0.0.1:{port}/big")).await;
        assert!(!big.is_error);
        assert!(big.content.len() < MAX_READ_OUTPUT + 200);
    }

    #[tokio::test]
    async fn test_redirect_to_a_private_address_is_refused_outside_tests() {
        // Without the test switch the address checks apply to every hop, so
        // a plain-http loopback URL never leaves the validator.
        let tools = ToolExecutor::new(PathBuf::from("/tmp"))
            .with_web_fetch_policy(WebFetchPolicy::AllowAll);
        let result = fetch(&tools, "http://127.0.0.1:9/ok".to_string()).await;
        assert!(result.is_error && result.content.contains("only https"));
        let metadata = fetch(&tools, "https://169.254.169.254/latest".to_string()).await;
        assert!(metadata.is_error && metadata.content.contains("refusing"));
    }
}
