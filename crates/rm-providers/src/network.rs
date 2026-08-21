//! Getting out of the building: proxies, and who to trust once through one.
//!
//! `ureq`'s free functions (`ureq::post` and friends) use a default agent that
//! reads no environment at all, and its `tls` feature verifies against
//! `webpki-roots` -- a fixed copy of Mozilla's list compiled into the binary --
//! rather than anything the machine is configured with. On a laptop that is
//! fine and is why it was not noticed. Behind a corporate proxy it fails twice
//! over: the request does not go through the proxy, and if it did, the proxy
//! re-terminates TLS with a certificate signed by an internal CA that no
//! Mozilla root vouches for, so verification rejects it.
//!
//! Both are configured the way every other tool on such a machine is
//! configured, through the environment, so that is what this reads.

use std::path::PathBuf;

use crate::ProviderError;

/// Where a request has to go, and what it should trust when it gets there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Network {
    /// The proxy URL, if one is configured.
    pub proxy: Option<String>,
    /// Hosts to reach directly, from `NO_PROXY`.
    pub no_proxy: Vec<String>,
    /// A PEM bundle to verify against *instead of* the built-in roots.
    pub ca_file: Option<PathBuf>,
}

impl Network {
    /// Read the conventional variables.
    ///
    /// Takes a getter rather than calling [`std::env::var`] so the rules can be
    /// tested without a process-wide environment, which no test can set without
    /// racing every other test in the binary.
    ///
    /// `HTTP_PROXY` is deliberately not read. It is the one variable in this
    /// family an untrusted party can set on you -- a CGI request carrying a
    /// `Proxy:` header arrives as `HTTP_PROXY` -- and this crate never needs it
    /// anyway, since `ALL_PROXY` covers a plain-HTTP provider and `HTTPS_PROXY`
    /// covers every other.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Network {
        // Uppercase first, then lowercase: both spellings are in use, and no
        // convention settles which wins, so this states its own.
        let first = |names: &[&str]| -> Option<String> {
            names
                .iter()
                .filter_map(|n| get(n))
                .map(|v| v.trim().to_string())
                .find(|v| !v.is_empty())
        };

        Network {
            proxy: first(&["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]),
            no_proxy: first(&["NO_PROXY", "no_proxy"])
                .map(|v| {
                    v.split(',')
                        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                        .filter(|e| !e.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            // `SSL_CERT_FILE` is the general spelling; `CURL_CA_BUNDLE` is
            // curl's, and is set on enough machines to be worth reading.
            ca_file: first(&["SSL_CERT_FILE", "CURL_CA_BUNDLE"]).map(PathBuf::from),
        }
    }

    /// Whether `host` should be reached directly.
    ///
    /// Loopback always is, whatever `NO_PROXY` says. Sending a request to your
    /// own machine out through a proxy and back is never what anyone configured
    /// a proxy for, it is how a local provider (an Ollama, a test double, this
    /// repo's own benchmark shim) breaks the moment a proxy appears in the
    /// environment, and the failure looks like the local server being down.
    ///
    /// CIDR entries are **not** matched -- `NO_PROXY=10.0.0.0/8` does not
    /// bypass `10.1.2.3`. Recognising them means writing a subnet matcher for
    /// both address families, and the entries that actually matter here are
    /// hostnames and loopback, which are handled. An unmatched entry costs a
    /// request through the proxy, not a wrong answer.
    pub fn bypasses(&self, host: &str) -> bool {
        let host = host
            .trim()
            .trim_matches(|c| c == '[' || c == ']')
            .to_lowercase();

        if host == "localhost" || host == "::1" || host.starts_with("127.") {
            return true;
        }

        self.no_proxy
            .iter()
            .any(|entry| entry == "*" || *entry == host || host.ends_with(&format!(".{entry}")))
    }

    /// An agent configured to reach `url`.
    ///
    /// Built once per provider rather than per request: the CA bundle is a file
    /// that has to be read and parsed, and on this machine it is 232KB, which
    /// is not a thing to do twice per remembered turn.
    pub fn agent(&self, url: &str) -> Result<ureq::Agent, ProviderError> {
        let mut builder = ureq::AgentBuilder::new().tls_config(self.tls_config()?);

        if let Some(proxy) = &self.proxy {
            let host = host_of(url);
            if !self.bypasses(&host) {
                // Checked here rather than left to `ureq::Proxy::new`, which
                // accepts almost anything -- "not a url", "://" and the empty
                // string all parse, and become a hostname nothing resolves. The
                // request then fails with "could not reach the provider", which
                // sends the reader looking at their network instead of at the
                // one environment variable that is wrong.
                //
                // A proxy that will not parse is a refusal, not a reason to go
                // direct: on a network where direct egress happens to work,
                // going direct quietly bypasses the policy the proxy exists to
                // enforce.
                check_proxy(proxy)?;
                let parsed = ureq::Proxy::new(proxy).map_err(|_| bad_proxy())?;
                builder = builder.proxy(parsed);
            }
        }

        Ok(builder.build())
    }

    /// The roots to verify against.
    ///
    /// With no `ca_file`, the compiled-in Mozilla set. With one, that file
    /// *instead* -- not in addition. Replacing is what `SSL_CERT_FILE` means
    /// everywhere else it is honoured, and adding to the defaults would quietly
    /// widen trust for someone who set it to narrow it.
    fn tls_config(&self) -> Result<std::sync::Arc<rustls::ClientConfig>, ProviderError> {
        let mut roots = rustls::RootCertStore::empty();

        match &self.ca_file {
            None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
            Some(path) => {
                let pem = std::fs::read(path).map_err(|e| {
                    // The path is named. Unlike a proxy URL it cannot carry a
                    // credential, and "which file" is the whole of what a
                    // reader needs to fix this.
                    ProviderError::Transport(format!(
                        "could not read the certificate bundle at {}: {e}",
                        path.display()
                    ))
                })?;

                let (added, ignored) = roots.add_parsable_certificates(
                    rustls_pemfile::certs(&mut pem.as_slice()).flatten(),
                );

                // A bundle that yields nothing is the dangerous case. Carrying
                // on would fall back to the built-in roots, which is exactly
                // the trust decision the file was set to override, and nothing
                // would say so -- it would look like it worked until it
                // mattered.
                if added == 0 {
                    return Err(ProviderError::Transport(format!(
                        "{} held no certificate this crate could use ({ignored} unusable). \
                         It should be PEM: a file of -----BEGIN CERTIFICATE----- blocks.",
                        path.display()
                    )));
                }
            }
        }

        Ok(std::sync::Arc::new(
            rustls::ClientConfig::builder_with_provider(
                // Named rather than taken from the process default, which is
                // whatever was installed first and panics if two crates
                // installed different ones.
                rustls::crypto::ring::default_provider().into(),
            )
            .with_safe_default_protocol_versions()
            .map_err(|e| ProviderError::Transport(format!("could not configure TLS: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth(),
        ))
    }
}

/// Refuse a proxy setting that cannot be one.
///
/// The value is never repeated back. `HTTPS_PROXY` legitimately carries
/// `user:password@`, and this message goes to a terminal and quite possibly an
/// issue tracker.
fn check_proxy(proxy: &str) -> Result<(), ProviderError> {
    let after_scheme = match proxy.split_once("://") {
        None => proxy,
        Some((scheme, rest)) => {
            // What `ureq` can actually carry out. Anything else is a typo or a
            // misunderstanding, and saying so beats connecting to a host named
            // after the mistake.
            if !matches!(
                scheme.to_lowercase().as_str(),
                "http" | "https" | "socks5" | "socks5h"
            ) {
                return Err(bad_proxy());
            }
            rest
        }
    };

    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);

    // A bracketed IPv6 literal keeps its colons; a bare one has more than one
    // and so cannot be carrying a port.
    let (host, port) = if let Some((bracketed, rest)) = host_port.split_once(']') {
        (bracketed.trim_start_matches('['), rest.strip_prefix(':'))
    } else if host_port.matches(':').count() > 1 {
        (host_port, None)
    } else {
        match host_port.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        }
    };

    // A host is a name or an address. Anything with a space in it -- "not a
    // url" is the one people actually type -- is neither, and `ureq` would take
    // it for a hostname and try to resolve it.
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '_' | '%'))
    {
        return Err(bad_proxy());
    }
    if let Some(port) = port {
        match port.parse::<u32>() {
            Ok(n) if (1..=65535).contains(&n) => {}
            _ => return Err(bad_proxy()),
        }
    }
    Ok(())
}

fn bad_proxy() -> ProviderError {
    ProviderError::Transport(
        "the proxy set in the environment is not a URL this crate can use. Expected \
         something like http://host:port, or http://user:pass@host:port, with a \
         scheme of http, https, socks5 or socks5h. The value is not repeated here \
         because it can carry a password."
            .to_string(),
    )
}

/// The host part of a URL, or an empty string if there is not one.
///
/// Hand-rolled rather than pulling in a URL parser: this crate needs the
/// authority and nothing else, and only to compare it against `NO_PROXY`.
fn host_of(url: &str) -> String {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    // Credentials, then the port -- taking the last '@' so a password
    // containing one does not truncate the host.
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    match host.rsplit_once(':') {
        // A bare IPv6 literal is full of colons; only strip a port when what
        // follows the last one is a port.
        Some((h, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            h.to_string()
        }
        _ => host.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Network {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Network::from_env(move |name| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        })
    }

    #[test]
    fn an_empty_environment_configures_nothing() {
        let n = env(&[]);
        assert_eq!(n, Network::default());
        assert!(n.proxy.is_none() && n.ca_file.is_none() && n.no_proxy.is_empty());
    }

    #[test]
    fn either_spelling_of_each_variable_is_read() {
        assert_eq!(
            env(&[("https_proxy", "http://p:8080")]).proxy.as_deref(),
            Some("http://p:8080")
        );
        assert_eq!(
            env(&[("HTTPS_PROXY", "http://p:8080")]).proxy.as_deref(),
            Some("http://p:8080")
        );
        assert_eq!(env(&[("no_proxy", "a.com")]).no_proxy, vec!["a.com"]);
    }

    #[test]
    fn uppercase_wins_and_all_proxy_is_the_fallback() {
        let n = env(&[
            ("HTTPS_PROXY", "http://upper:1"),
            ("https_proxy", "http://lower:2"),
            ("ALL_PROXY", "http://all:3"),
        ]);
        assert_eq!(n.proxy.as_deref(), Some("http://upper:1"));
        assert_eq!(
            env(&[("ALL_PROXY", "http://all:3")]).proxy.as_deref(),
            Some("http://all:3")
        );
    }

    #[test]
    fn http_proxy_is_not_read_at_all() {
        // The httpoxy variable: a CGI request's `Proxy:` header arrives under
        // this name, so honouring it lets a caller redirect our traffic.
        assert!(env(&[("HTTP_PROXY", "http://attacker:1")]).proxy.is_none());
        assert!(env(&[("http_proxy", "http://attacker:1")]).proxy.is_none());
    }

    #[test]
    fn a_variable_set_to_nothing_is_the_same_as_unset() {
        // Set-but-empty is how a shell says "no proxy here", and reading it as
        // a proxy URL would make every request fail.
        assert!(env(&[("HTTPS_PROXY", "")]).proxy.is_none());
        assert!(env(&[("HTTPS_PROXY", "   ")]).proxy.is_none());
        assert!(
            env(&[("HTTPS_PROXY", ""), ("https_proxy", "http://p:1")])
                .proxy
                .as_deref()
                == Some("http://p:1"),
            "an empty first choice falls through to the next"
        );
    }

    #[test]
    fn ssl_cert_file_is_preferred_and_curl_s_spelling_is_the_fallback() {
        assert_eq!(
            env(&[("SSL_CERT_FILE", "/a.pem"), ("CURL_CA_BUNDLE", "/b.pem")]).ca_file,
            Some(PathBuf::from("/a.pem"))
        );
        assert_eq!(
            env(&[("CURL_CA_BUNDLE", "/b.pem")]).ca_file,
            Some(PathBuf::from("/b.pem"))
        );
    }

    #[test]
    fn loopback_is_always_direct_however_the_environment_is_set() {
        // The case that breaks a local provider the moment a proxy appears.
        let n = env(&[("HTTPS_PROXY", "http://p:8080")]);
        assert!(n.no_proxy.is_empty());
        for host in ["localhost", "127.0.0.1", "127.53.0.1", "::1"] {
            assert!(n.bypasses(host), "{host} should never go through a proxy");
        }
        assert!(!n.bypasses("api.openai.com"));
    }

    #[test]
    fn no_proxy_matches_a_host_and_its_subdomains_but_not_a_suffix_of_a_word() {
        let n = env(&[("NO_PROXY", "example.com, .internal ,localhost")]);
        assert!(n.bypasses("example.com"));
        assert!(n.bypasses("api.example.com"));
        assert!(n.bypasses("thing.internal"));
        // A leading dot on the entry is optional, and stripped either way.
        assert_eq!(n.no_proxy, vec!["example.com", "internal", "localhost"]);
        // "notexample.com" ends with "example.com" as a string and is a
        // different domain.
        assert!(!n.bypasses("notexample.com"));
        assert!(!n.bypasses("example.com.evil.net"));
    }

    #[test]
    fn a_star_bypasses_everything() {
        assert!(env(&[("NO_PROXY", "*")]).bypasses("api.openai.com"));
    }

    #[test]
    fn a_cidr_entry_does_not_match_and_that_is_documented_rather_than_silent() {
        // Pinned so the limitation is a decision with a test rather than a
        // surprise: this address goes through the proxy.
        let n = env(&[("NO_PROXY", "10.0.0.0/8")]);
        assert!(!n.bypasses("10.1.2.3"));
    }

    #[test]
    fn the_host_is_found_whatever_the_url_carries_around_it() {
        assert_eq!(host_of("https://api.openai.com/v1/chat"), "api.openai.com");
        assert_eq!(host_of("https://api.openai.com:8443/v1"), "api.openai.com");
        assert_eq!(host_of("http://127.0.0.1:8731/v1"), "127.0.0.1");
        assert_eq!(host_of("https://user:pw@host.com/v1"), "host.com");
        // A password with an '@' in it: the host is after the *last* one.
        assert_eq!(host_of("https://user:p@ss@host.com/v1"), "host.com");
        assert_eq!(host_of("api.openai.com/v1"), "api.openai.com");
        assert_eq!(host_of("https://[::1]:8080/v1"), "[::1]");
        assert_eq!(host_of(""), "");
    }

    #[test]
    fn a_bracketed_ipv6_loopback_is_recognised_through_the_brackets() {
        let n = env(&[("HTTPS_PROXY", "http://p:1")]);
        assert!(n.bypasses(&host_of("http://[::1]:8080/v1")));
    }

    #[test]
    fn a_missing_certificate_bundle_names_the_file() {
        let n = Network {
            ca_file: Some(PathBuf::from("/no/such/bundle.pem")),
            ..Network::default()
        };
        let Err(ProviderError::Transport(why)) = n.agent("https://api.openai.com/v1") else {
            panic!("a bundle that is not there cannot be trusted");
        };
        assert!(why.contains("/no/such/bundle.pem"), "{why}");
    }

    #[test]
    fn a_bundle_holding_no_certificate_is_refused_rather_than_silently_ignored() {
        // The dangerous case: carrying on would verify against the built-in
        // roots, which is the decision the file was set to override.
        let dir = std::env::temp_dir().join("rm-providers-empty-bundle-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-a-bundle.pem");
        std::fs::write(&path, b"this file is not a certificate\n").unwrap();

        let n = Network {
            ca_file: Some(path.clone()),
            ..Network::default()
        };
        let Err(ProviderError::Transport(why)) = n.agent("https://api.openai.com/v1") else {
            panic!("an empty bundle cannot be trusted");
        };
        assert!(why.contains("no certificate"), "{why}");
        assert!(
            why.contains("PEM"),
            "the fix has to be in the message: {why}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_proxy_url_that_will_not_parse_is_refused_without_repeating_it() {
        // Whatever is in the variable stays in the variable: `HTTPS_PROXY`
        // legitimately carries `user:password@`, and this message goes to a
        // terminal and quite possibly an issue tracker.
        //
        // The value has a space in it because that is what makes it refusable.
        // A credential with no space -- "sk-proj-ABC123" -- is a syntactically
        // valid single-label hostname, indistinguishable from "proxy" or
        // "squid", and refusing it would refuse those. It is instead used as a
        // hostname, fails to resolve, and `transport_failure` reports that in
        // this crate's own words without quoting `ureq`.
        const CREDENTIAL: &str = "sk-proj-SECRET-IN-A-BROKEN-VALUE not a url";
        let n = Network {
            proxy: Some(CREDENTIAL.to_string()),
            ..Network::default()
        };
        let Err(ProviderError::Transport(why)) = n.agent("https://api.openai.com/v1") else {
            panic!("an unusable proxy is a refusal, not a reason to go direct");
        };
        assert!(!why.contains(CREDENTIAL), "the value came back out: {why}");
        assert!(
            !why.contains("sk-proj-SECRET-IN-A-BROKEN-VALUE"),
            "part of the value came back out: {why}"
        );
        assert!(why.contains("http://host:port"), "{why}");
    }

    #[test]
    fn a_single_label_hostname_is_a_proxy_and_not_a_typo() {
        // The other side of the test above: these are what a proxy on a
        // corporate network is usually called, and none has a dot in it.
        for name in ["proxy", "squid", "gateway-1", "corp_proxy"] {
            assert_eq!(check_proxy(name), Ok(()), "{name:?}");
        }
    }

    #[test]
    fn the_proxy_forms_that_are_real_are_all_accepted() {
        for good in [
            "http://p:8080",
            "https://p:8080",
            "socks5://p:1080",
            "socks5h://p:1080",
            "http://user:pass@p:8080",
            "http://[::1]:8080",
            "p:8080",
            "proxy.internal",
            "HTTP://P:8080",
        ] {
            assert_eq!(
                check_proxy(good),
                Ok(()),
                "{good:?} is a real proxy setting"
            );
        }
    }

    #[test]
    fn the_values_ureq_would_have_taken_for_a_hostname_are_refused_here() {
        // Every one of these is accepted by `ureq::Proxy::new` and becomes a
        // host nothing resolves, so the failure arrives as "could not reach the
        // provider" and points at the wrong thing.
        for bad in [
            "not a url",
            "://",
            "",
            "   ",
            "ftp://p:1",
            "http://:8080",
            "http://p:notaport",
            "http://p:0",
            "http://p:70000",
        ] {
            assert!(
                ureq::Proxy::new(bad).is_ok() || bad.starts_with("ftp"),
                "{bad:?} is only worth checking because ureq accepts it"
            );
            assert!(check_proxy(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn a_bad_proxy_is_not_reached_for_at_all_when_the_host_bypasses_it() {
        // Local providers keep working on a machine whose proxy setting is
        // broken, because the proxy is never consulted for them.
        let n = Network {
            proxy: Some("not a url".to_string()),
            ..Network::default()
        };
        assert!(n.agent("http://127.0.0.1:8731/v1").is_ok());
    }

    #[test]
    fn with_nothing_configured_an_agent_is_still_built() {
        assert!(Network::default()
            .agent("https://api.openai.com/v1")
            .is_ok());
    }
}
