/// Validates existance of Interfaces and IP Addresses

use std::net::IpAddr;

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

/// Represents an Interface on running device
#[derive(Debug)]
pub struct LocalInterface {
    pub ip:   IpAddr,
    pub name: Option<String>
}

/// Represents IP Addresses on a given interface
#[derive(Debug)]
pub struct LocalIpAddresses {
    pub ipv4_addrs: Vec<LocalInterface>,
    pub ipv6_addrs: Vec<LocalInterface>
}

/// Search interface for an available IP Address - both versions 4 and 6 
///  - Returns LocalIpAddresses struct
pub async fn get_local_ips() -> LocalIpAddresses {
    trace!("Searching for network interfaces and IP addresses");

    let mut ipv4_addrs = Vec::new();
    let mut ipv6_addrs = Vec::new();

    let addrs = crate::net::interfaces::local_addrs().unwrap_or_else(|e| {
        error!("Failed to enumerate local interfaces: {}", e);
        Vec::new()
    });
    for a in addrs {
        trace!("Found {:?} on interface {:?}", a.ip, a.interface);
        let entry = LocalInterface { ip: a.ip, name: Some(a.interface) };
        match a.ip {
            IpAddr::V4(_) => ipv4_addrs.push(entry),
            IpAddr::V6(_) => ipv6_addrs.push(entry),
        }
    }
    LocalIpAddresses {
        ipv4_addrs: ipv4_addrs,
        ipv6_addrs: ipv6_addrs
    }
}

/// Used with ip-start in Listen, if several IP addresses of same version
/// available 
pub async fn ipstr_starts_with(
    ip: & IpAddr, starting_octets: & Option<String>
) -> bool {
    match starting_octets {
        Some(start) => ip.to_string().starts_with(& * start),
        None => true,
    }
}

/// Returns IP addresses on an interface
///  - Listen - uses starting_octets to return a unique address
pub async fn get_matching_ipstr(
    ips: & Vec<LocalInterface>,
    iname: & Option<String>, starting_octets: & Option<String>
) -> Vec<String> {
    match starting_octets {
        Some(ip) => trace!(
            "Selecting IP addresses starting with: {:?} on interface: {:?}",
            ip, iname
        ),
        None => trace!(
            "Selecting all IP addresses on interface: {:?}",
            iname
        )
    }

    let mut ipstr = Vec::new();
    // let target_name = Some(iname.to_string());

    for ip in ips {
        if *iname == None || ip.name == *iname || ip.name == None {
            if ! ipstr_starts_with(& ip.ip, starting_octets).await{continue;}
            trace!("Address {:?} is a positive match", ip.ip);
            ipstr.push(ip.ip.to_string())
        }
    }

    ipstr
}
