//! Local interface and address enumeration, plus the filters the CLI exposes
//! (`--name`, `--ip-start`, `--ip-version`) for choosing *the* local address a
//! party advertises to the broker.

use std::net::IpAddr;

use crate::{Error, Result};

/// IP address family selector.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum, serde::Serialize, serde::Deserialize,
)]
pub enum IpVersion {
    /// IPv4 only.
    #[value(name = "4", alias = "v4", alias = "ipv4")]
    V4,
    /// IPv6 only.
    #[value(name = "6", alias = "v6", alias = "ipv6")]
    V6,
}

impl IpVersion {
    /// Whether `ip` belongs to this family.
    pub fn matches(self, ip: IpAddr) -> bool {
        match self {
            IpVersion::V4 => ip.is_ipv4(),
            IpVersion::V6 => ip.is_ipv6(),
        }
    }
}

/// One address assigned to one local interface.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocalAddr {
    /// Interface name as the OS reports it (`eth0`, `en0`, `lo`).
    pub interface: String,
    /// The address.
    pub ip: IpAddr,
}

/// Filters that narrow [`local_addrs`] down to the address a party should use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selector {
    /// Only addresses on this interface.
    pub interface: Option<String>,
    /// Only addresses whose textual form starts with this prefix
    /// (`10.128.` selects one subnet, `fe80:` selects link-local IPv6).
    pub prefix: Option<String>,
    /// Only this address family.
    pub version: Option<IpVersion>,
}

impl Selector {
    /// True when `addr` passes every configured filter.
    pub fn matches(&self, addr: &LocalAddr) -> bool {
        if let Some(name) = &self.interface {
            if &addr.interface != name {
                return false;
            }
        }
        if let Some(v) = self.version {
            if !v.matches(addr.ip) {
                return false;
            }
        }
        if let Some(p) = &self.prefix {
            if !addr.ip.to_string().starts_with(p.as_str()) {
                return false;
            }
        }
        true
    }

    /// All addresses passing the filters, in input order.
    pub fn filter<'a>(&self, addrs: impl IntoIterator<Item = &'a LocalAddr>) -> Vec<LocalAddr> {
        addrs
            .into_iter()
            .filter(|a| self.matches(a))
            .cloned()
            .collect()
    }

    /// Exactly one address must pass the filters; anything else is
    /// [`Error::AmbiguousAddress`] with the candidates listed so the user can
    /// tighten `--name` or `--ip-start`.
    pub fn select_one<'a>(
        &self,
        addrs: impl IntoIterator<Item = &'a LocalAddr>,
    ) -> Result<LocalAddr> {
        let mut found = self.filter(addrs);
        if found.len() == 1 {
            return Ok(found.remove(0));
        }
        Err(Error::AmbiguousAddress {
            found: found.len(),
            candidates: found
                .iter()
                .map(|a| format!("{} on {}", a.ip, a.interface))
                .collect(),
        })
    }
}

/// Every address on every interface of this host, sorted by interface name
/// and then by address so output is stable.
pub fn local_addrs() -> Result<Vec<LocalAddr>> {
    let mut out: Vec<LocalAddr> = if_addrs::get_if_addrs()?
        .into_iter()
        .map(|i| LocalAddr {
            ip: i.ip(),
            interface: i.name,
        })
        .collect();
    out.sort_by(|a, b| a.interface.cmp(&b.interface).then_with(|| a.ip.cmp(&b.ip)));
    out.dedup();
    Ok(out)
}

/// Distinct interface names carrying at least one address of `version`
/// (`None` for any family), in first-seen order.
pub fn interface_names<'a>(
    addrs: impl IntoIterator<Item = &'a LocalAddr>,
    version: Option<IpVersion>,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for a in addrs {
        if version.is_some_and(|v| !v.matches(a.ip)) {
            continue;
        }
        if !names.contains(&a.interface) {
            names.push(a.interface.clone());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<LocalAddr> {
        let mk = |i: &str, ip: &str| LocalAddr {
            interface: i.into(),
            ip: ip.parse().unwrap(),
        };
        vec![
            mk("lo", "127.0.0.1"),
            mk("lo", "::1"),
            mk("en0", "10.128.0.5"),
            mk("en0", "fe80::1"),
            mk("en1", "10.129.0.9"),
        ]
    }

    #[test]
    fn version_filter() {
        let s = Selector {
            version: Some(IpVersion::V4),
            ..Default::default()
        };
        let got = s.filter(&sample());
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|a| a.ip.is_ipv4()));
    }

    #[test]
    fn interface_and_prefix_filters_combine() {
        let s = Selector {
            interface: Some("en0".into()),
            prefix: Some("10.".into()),
            version: None,
        };
        let one = s.select_one(&sample()).unwrap();
        assert_eq!(one.ip.to_string(), "10.128.0.5");
    }

    #[test]
    fn ambiguity_is_an_error_listing_candidates() {
        let s = Selector {
            version: Some(IpVersion::V4),
            prefix: Some("10.".into()),
            ..Default::default()
        };
        match s.select_one(&sample()) {
            Err(Error::AmbiguousAddress { found, candidates }) => {
                assert_eq!(found, 2);
                assert!(candidates.iter().any(|c| c.contains("en1")));
            }
            other => panic!("unexpected {other:?}"),
        }
        let none = Selector {
            interface: Some("nope".into()),
            ..Default::default()
        };
        assert!(matches!(
            none.select_one(&sample()),
            Err(Error::AmbiguousAddress { found: 0, .. })
        ));
    }

    #[test]
    fn interface_names_dedup_and_respect_version() {
        assert_eq!(interface_names(&sample(), None), vec!["lo", "en0", "en1"]);
        assert_eq!(
            interface_names(&sample(), Some(IpVersion::V6)),
            vec!["lo", "en0"]
        );
    }

    #[test]
    fn host_has_a_loopback_address() {
        let addrs = local_addrs().unwrap();
        assert!(addrs.iter().any(|a| a.ip.is_loopback()), "{addrs:?}");
        let sorted = {
            let mut c = addrs.clone();
            c.sort_by(|a, b| a.interface.cmp(&b.interface).then_with(|| a.ip.cmp(&b.ip)));
            c
        };
        assert_eq!(addrs, sorted);
    }

    #[test]
    fn ip_version_value_names() {
        use clap::ValueEnum;
        assert_eq!(IpVersion::V4.to_possible_value().unwrap().get_name(), "4");
        assert_eq!(IpVersion::from_str("ipv6", true).unwrap(), IpVersion::V6);
    }
}
