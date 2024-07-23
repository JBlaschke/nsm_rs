/// Validates existance of Interfaces and IP Addresses

use pnet::datalink;
use pnet::ipnetwork::IpNetwork;
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
pub fn get_local_ips() -> LocalIpAddresses {
    trace!("Searching for network interfaces and IP addresses");

    let mut ipv4_addrs = Vec::new();
    let mut ipv6_addrs = Vec::new();

    for iface in datalink::interfaces() {
        trace!("Found interface: {:?}", & iface.name);

        for ip_network in iface.ips {
            match ip_network {
                IpNetwork::V4(ipv4_network) => {
                    trace!("Found IPv4 address: {:?}", ipv4_network.ip());

                    ipv4_addrs.push(
                        LocalInterface {
                            ip:   IpAddr::V4(ipv4_network.ip()),
                            name: Some(iface.name.clone())
                        }
                    )
                }
                IpNetwork::V6(ipv6_network) => {
                    trace!("Found IPv6 address: {:?}", ipv6_network.ip());

                    ipv6_addrs.push(
                        LocalInterface {
                            ip:   IpAddr::V6(ipv6_network.ip()),
                            name: Some(iface.name.clone())
                        }
                    )
                }
            }
        }
    }
    LocalIpAddresses {
        ipv4_addrs: ipv4_addrs,
        ipv6_addrs: ipv6_addrs
    }
}

/// Used with ip-start in Listen, if several IP addresses of same version available 
pub fn ipstr_starts_with(
    ip: & IpAddr, starting_octets: & Option<String>
) -> bool {
    match starting_octets {
        Some(start) => ip.to_string().starts_with(& * start),
        None => true,
    }
}

/// Returns IP addresses on an interface
///  - Listen - uses starting_octets to return a unique address
pub fn get_matching_ipstr(
    ips: & Vec<LocalInterface>,
    interface_name: & str, starting_octets: & Option<String>
) -> Vec<String> {
    match starting_octets {
        Some(ip) => trace!(
            "Selecting IP addresses starting with: {:?} on interface: {:?}",
            ip, interface_name
        ),
        None => trace!(
            "Selecting all IP addresses on interface: {:?}",
            interface_name
        )
    }

    let mut ipstr = Vec::new();
    let target_name = Some(interface_name.to_string());

    for ip in ips {
        if ip.name == target_name || ip.name == None {
            if ! ipstr_starts_with(& ip.ip, starting_octets){continue;}
            trace!("Address {:?} is a positive match", ip.ip);
            ipstr.push(ip.ip.to_string())
        }
    }

    ipstr
}