//! The address grammar shared by the CLI, the REST control plane and the wire
//! protocol.
//!
//! ```text
//! host:port            raw TCP (also tcp://host:port)
//! tls://host:port      TCP with TLS
//! http://host:port     HTTP
//! https://host:port    HTTPS
//! ```
//!
//! `host` may be a DNS name, an IPv4 literal, or an IPv6 literal either in
//! brackets (`[::1]:8080`) or bare (`::1:8080`, in which case the last colon
//! separates the port, as the previous implementation accepted). A single
//! trailing slash is tolerated; any other path is an error. The port is
//! mandatory.

use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// How to reach a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    /// Length-prefixed JSON messages over a plain TCP connection.
    Tcp,
    /// [`Transport::Tcp`] over TLS.
    Tls,
    /// JSON messages as HTTP request and response bodies.
    Http,
    /// [`Transport::Http`] over TLS.
    Https,
}

impl Transport {
    /// URL scheme, or `None` for raw TCP (whose textual form is a bare
    /// `host:port`).
    pub fn scheme(self) -> Option<&'static str> {
        match self {
            Transport::Tcp => None,
            Transport::Tls => Some("tls"),
            Transport::Http => Some("http"),
            Transport::Https => Some("https"),
        }
    }

    /// True for transports that speak HTTP.
    pub fn is_http(self) -> bool {
        matches!(self, Transport::Http | Transport::Https)
    }

    /// True when the transport itself implies TLS.
    pub fn is_tls(self) -> bool {
        matches!(self, Transport::Tls | Transport::Https)
    }

    /// The TLS or plain variant of the same family.
    pub fn with_tls(self, tls: bool) -> Transport {
        match (self.is_http(), tls) {
            (false, false) => Transport::Tcp,
            (false, true) => Transport::Tls,
            (true, false) => Transport::Http,
            (true, true) => Transport::Https,
        }
    }
}

/// A peer address: transport, host and port.
///
/// Serialises as its textual form (`"tls://10.0.0.5:12000"`), so the same
/// string works on the command line, in REST bodies and on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Addr {
    /// Transport used to reach the peer.
    pub transport: Transport,
    /// Host name or IP literal, without brackets.
    pub host: String,
    /// TCP port.
    pub port: u16,
}

/// Returned when a string is not a valid [`Addr`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid address {input:?}: {reason} (expected host:port, http://host:port or https://host:port)"
)]
pub struct ParseAddrError {
    /// The offending input.
    pub input: String,
    /// What was wrong with it.
    pub reason: &'static str,
}

impl Addr {
    /// Build an address from parts.
    pub fn new(transport: Transport, host: impl Into<String>, port: u16) -> Self {
        Addr {
            transport,
            host: host.into(),
            port,
        }
    }

    /// Shorthand for a raw TCP address.
    pub fn tcp(host: impl Into<String>, port: u16) -> Self {
        Addr::new(Transport::Tcp, host, port)
    }

    /// Same host and transport, different port.
    pub fn with_port(&self, port: u16) -> Self {
        Addr::new(self.transport, self.host.clone(), port)
    }

    /// Same host and port, different transport.
    pub fn with_transport(&self, transport: Transport) -> Self {
        Addr::new(transport, self.host.clone(), self.port)
    }

    /// `host:port` with IPv6 literals bracketed, suitable for `ToSocketAddrs`
    /// and for URL authorities.
    pub fn authority(&self) -> String {
        match self.host.parse::<IpAddr>() {
            Ok(IpAddr::V6(v6)) => format!("[{v6}]:{}", self.port),
            _ => format!("{}:{}", self.host, self.port),
        }
    }

    /// The host as an IP literal, if it is one.
    pub fn ip(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }

    /// The address as a socket address, if the host is an IP literal. DNS
    /// names need [`Addr::resolve`].
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        self.ip().map(|ip| SocketAddr::new(ip, self.port))
    }

    /// Resolve the host (IP literal or DNS name) to the first socket address.
    pub async fn resolve(&self) -> crate::Result<SocketAddr> {
        if let Some(sa) = self.socket_addr() {
            return Ok(sa);
        }
        tokio::net::lookup_host(self.authority())
            .await?
            .next()
            .ok_or_else(|| crate::Error::Resolve(self.to_string()))
    }

    /// For HTTP transports, the URL of `path` on this peer (`path` with or
    /// without a leading slash). `None` for raw TCP.
    pub fn url(&self, path: &str) -> Option<String> {
        let scheme = self.transport.scheme()?;
        let path = path.trim_start_matches('/');
        Some(format!("{scheme}://{}/{path}", self.authority()))
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.transport.scheme() {
            Some(scheme) => write!(f, "{scheme}://{}", self.authority()),
            None => f.write_str(&self.authority()),
        }
    }
}

impl Serialize for Addr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Addr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl FromStr for Addr {
    type Err = ParseAddrError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let err = |reason| ParseAddrError {
            input: input.to_owned(),
            reason,
        };
        let s = input.trim();
        if s.is_empty() {
            return Err(err("empty string"));
        }

        let (transport, rest) = if let Some(r) = s.strip_prefix("https://") {
            (Transport::Https, r)
        } else if let Some(r) = s.strip_prefix("http://") {
            (Transport::Http, r)
        } else if let Some(r) = s.strip_prefix("tcp://") {
            (Transport::Tcp, r)
        } else if let Some(r) = s.strip_prefix("tls://") {
            (Transport::Tls, r)
        } else if s.contains("://") {
            return Err(err("unsupported scheme"));
        } else {
            (Transport::Tcp, s)
        };

        // Tolerate exactly one trailing slash (as in a pasted URL); anything
        // else after the authority is a path we cannot represent.
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.contains('/') {
            return Err(err("a path is not allowed"));
        }
        if rest.contains(['@', '?', '#']) {
            return Err(err("userinfo, query and fragment are not allowed"));
        }

        let (host, port_str) = if let Some(after_bracket) = rest.strip_prefix('[') {
            // Bracketed IPv6: [v6]:port
            let (inner, tail) = after_bracket
                .split_once(']')
                .ok_or_else(|| err("unterminated '[' in IPv6 literal"))?;
            let port_str = tail
                .strip_prefix(':')
                .ok_or_else(|| err("port is required"))?;
            if inner.parse::<IpAddr>().map(|ip| ip.is_ipv6()) != Ok(true) {
                return Err(err("brackets must contain an IPv6 literal"));
            }
            (inner, port_str)
        } else {
            // The last colon separates the port, so bare IPv6 literals work.
            rest.rsplit_once(':')
                .ok_or_else(|| err("port is required"))?
        };

        if host.is_empty() {
            return Err(err("host is empty"));
        }
        let port: u16 = port_str
            .parse()
            .map_err(|_| err("port must be a number between 0 and 65535"))?;

        // IP literals are stored in canonical form (`::1`, not `0::0001`) so
        // two spellings of one address compare equal and display alike.
        let host = match host.parse::<IpAddr>() {
            Ok(ip) => ip.to_string(),
            Err(_) => host.to_owned(),
        };
        Ok(Addr {
            transport,
            host,
            port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Addr {
        s.parse().unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn plain_host_port_is_tcp() {
        let a = parse("10.0.0.5:12000");
        assert_eq!(a, Addr::tcp("10.0.0.5", 12000));
        assert_eq!(a.to_string(), "10.0.0.5:12000");
        assert_eq!(a.socket_addr(), Some("10.0.0.5:12000".parse().unwrap()));
    }

    #[test]
    fn schemes() {
        assert_eq!(parse("http://broker:80").transport, Transport::Http);
        assert_eq!(parse("https://broker:443/").transport, Transport::Https);
        assert_eq!(parse("tcp://broker:1").transport, Transport::Tcp);
        assert_eq!(parse("tls://broker:1").transport, Transport::Tls);
        assert_eq!(parse("tls://broker:1").to_string(), "tls://broker:1");
        assert!(Transport::Tls.is_tls() && Transport::Https.is_tls());
        assert!(!Transport::Tcp.is_tls() && !Transport::Http.is_tls());
        assert_eq!(Transport::Tcp.with_tls(true), Transport::Tls);
        assert_eq!(Transport::Https.with_tls(false), Transport::Http);
        assert_eq!(Transport::Tls.with_tls(false), Transport::Tcp);
        assert_eq!(
            parse("https://broker:443/").to_string(),
            "https://broker:443"
        );
        assert_eq!(
            parse("http://a:1").url("v1/message"),
            Some("http://a:1/v1/message".into())
        );
        assert_eq!(
            parse("http://a:1").url("/v1/message"),
            Some("http://a:1/v1/message".into())
        );
        assert_eq!(parse("a:1").url("x"), None);
    }

    #[test]
    fn dns_names_are_kept() {
        let a = parse("broker.nersc.gov:12000");
        assert_eq!(a.host, "broker.nersc.gov");
        assert_eq!(a.ip(), None);
        assert_eq!(a.socket_addr(), None);
    }

    #[test]
    fn ipv6_bracketed_and_bare() {
        let b = parse("[::1]:8080");
        let bare = parse("::1:8080");
        assert_eq!(b.host, "::1");
        assert_eq!(b, bare);
        assert_eq!(b.to_string(), "[::1]:8080");
        assert_eq!(b.authority(), "[::1]:8080");
        assert_eq!(
            parse("http://[fe80::1]:9/").to_string(),
            "http://[fe80::1]:9"
        );
        assert_eq!(b.socket_addr(), Some("[::1]:8080".parse().unwrap()));
    }

    #[test]
    fn display_round_trips() {
        for s in [
            "1.2.3.4:5",
            "host:65535",
            "http://host:80",
            "https://[::1]:443",
            "tls://host:12000",
            "[2001:db8::1]:1",
        ] {
            let a = parse(s);
            assert_eq!(parse(&a.to_string()), a, "{s}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        for s in [
            "",
            "host",
            "host:",
            ":80",
            "host:port",
            "host:70000",
            "host:-1",
            "http://host",
            "http://host:80/path",
            "ftp://host:21",
            "user@host:80",
            "host:80?x=1",
            "[::1:80",
            "[not-v6]:80",
        ] {
            assert!(s.parse::<Addr>().is_err(), "{s:?} should be rejected");
        }
    }

    #[test]
    fn error_message_names_the_input() {
        let e = "nope".parse::<Addr>().unwrap_err();
        assert!(e.to_string().contains("nope"));
        assert!(e.to_string().contains("port is required"));
    }

    #[test]
    fn transport_serde_is_lowercase() {
        assert_eq!(
            serde_json::to_string(&Transport::Https).unwrap(),
            "\"https\""
        );
    }

    #[test]
    fn addr_serde_is_the_textual_form() {
        let a = Addr::new(Transport::Tls, "::1", 12000);
        assert_eq!(serde_json::to_string(&a).unwrap(), "\"tls://[::1]:12000\"");
        let back: Addr = serde_json::from_str("\"tls://[::1]:12000\"").unwrap();
        assert_eq!(back, a);
        let plain: Addr = serde_json::from_str("\"h:1\"").unwrap();
        assert_eq!(plain, Addr::tcp("h", 1));
        assert!(serde_json::from_str::<Addr>("\"nope\"").is_err());
        assert!(serde_json::from_str::<Addr>(r#"{"host":"h","port":1}"#).is_err());
    }

    #[tokio::test]
    async fn resolve_literal_and_localhost() {
        assert_eq!(
            parse("127.0.0.1:9").resolve().await.unwrap(),
            "127.0.0.1:9".parse::<SocketAddr>().unwrap()
        );
        let lo = parse("localhost:9").resolve().await.unwrap();
        assert!(lo.ip().is_loopback());
    }

    // ---- randomized properties (deterministic seeds; see crate::testing) ----

    #[test]
    fn generated_addresses_round_trip_through_display_parse_and_serde() {
        let mut rng = crate::testing::Rng::new(2024);
        for i in 0..3000 {
            let addr = rng.addr();
            let text = addr.to_string();
            let parsed: Addr = text
                .parse()
                .unwrap_or_else(|e| panic!("iteration {i}: {text:?} does not parse: {e}"));
            assert_eq!(parsed, addr, "iteration {i}: {text:?}");
            let json = serde_json::to_string(&addr).unwrap();
            assert_eq!(json, serde_json::to_string(&text).unwrap(), "iteration {i}");
            assert_eq!(
                serde_json::from_str::<Addr>(&json).unwrap(),
                addr,
                "iteration {i}"
            );
            // The authority is what the wire and URLs carry; IPv6 is bracketed.
            let authority = addr.authority();
            assert!(text.ends_with(&authority), "iteration {i}: {text:?}");
            assert_eq!(
                authority.starts_with('['),
                addr.ip().is_some_and(|ip| ip.is_ipv6()),
                "iteration {i}: {authority:?}"
            );
            // Only raw TCP has no scheme and therefore no URL form.
            match addr.url("v1/message") {
                Some(url) => {
                    assert_ne!(addr.transport, Transport::Tcp, "iteration {i}");
                    assert_eq!(url, format!("{text}/v1/message"), "iteration {i}");
                }
                None => assert_eq!(addr.transport, Transport::Tcp, "iteration {i}"),
            }
        }
    }

    #[test]
    fn ip_literals_are_canonicalised() {
        let a: Addr = "[0:0:0:0:0:0:0:1]:80".parse().unwrap();
        let b: Addr = "::1:80".parse().unwrap();
        assert_eq!(a, b);
        assert_eq!(a.host, "::1");
        assert_eq!(a.to_string(), "[::1]:80");
        let v4: Addr = "http://127.000.000.001:80".parse().unwrap_or_else(|_| {
            // Leading zeros are not valid IPv4 text: kept as a name, unchanged.
            "http://127.000.000.001:80".parse().unwrap()
        });
        assert_eq!(v4.host, "127.000.000.001");
        assert_eq!(v4.ip(), None);
        let name: Addr = "Broker.Example:12000".parse().unwrap();
        assert_eq!(name.host, "Broker.Example", "names are not case-folded");
    }

    #[test]
    fn parser_never_panics_and_accepted_forms_are_canonical() {
        const ALPHABET: &[char] = &[
            'a', 'Z', '0', '9', '-', '.', ':', '/', '[', ']', '@', '?', '#', ' ', 'é', '%', '+',
        ];
        const PREFIXES: &[&str] = &["http://", "https://", "tls://", "tcp://", "ftp://", "//"];
        let mut rng = crate::testing::Rng::new(7);
        let (mut accepted, mut rejected) = (0, 0);
        for i in 0..8000 {
            // Half pure noise, half almost-valid: a plausible host and port
            // with a few random characters thrown in.
            let mut input = if rng.chance(2) {
                rng.string(ALPHABET, 24)
            } else {
                let mut host = rng.host();
                if rng.chance(3) {
                    let at = rng.below(host.len() + 1);
                    host.insert(at, *rng.pick(ALPHABET));
                }
                if rng.chance(4) {
                    host = format!("[{host}]");
                }
                let port = match rng.below(4) {
                    0 => String::new(),
                    1 => rng.string(&['0', '1', '9', 'x', '+', '-'], 6),
                    _ => (rng.next_u64() as u16).to_string(),
                };
                let tail = if rng.chance(4) {
                    *rng.pick(&["/", "//", "/x", "?q"])
                } else {
                    ""
                };
                format!("{host}:{port}{tail}")
            };
            if rng.chance(3) {
                input = format!("{}{input}", rng.pick(PREFIXES));
            }
            match input.parse::<Addr>() {
                Ok(addr) => {
                    accepted += 1;
                    let text = addr.to_string();
                    let again: Addr = text.parse().unwrap_or_else(|e| {
                        panic!("iteration {i}: {input:?} -> {text:?} does not re-parse: {e}")
                    });
                    assert_eq!(again, addr, "iteration {i}: {input:?} -> {text:?}");
                    assert!(!addr.host.is_empty(), "iteration {i}");
                    assert!(!addr.host.contains(['/', '@', '?', '#']), "iteration {i}");
                }
                Err(e) => {
                    rejected += 1;
                    let shown = e.to_string();
                    assert!(shown.contains(input.trim()), "iteration {i}: {shown:?}");
                }
            }
        }
        assert!(
            accepted > 100 && rejected > 100,
            "{accepted} accepted, {rejected} rejected"
        );
    }
}
