//! The host's network, for Pro DJ Link, the NFS and dbserver servers and
//! stemd: which interfaces it has, a UDP socket that shares its port with
//! other Link tools, and mDNS.
//!
//! Everything above this crate uses `std::net` for the rest, and none of
//! it asks what it runs on.  This crate is the one place that does: on a
//! native OS it is `os.rs`; in a browser (wasm32) it is `none.rs`, a host
//! with no interfaces where every socket refuses, so the deck runs with no
//! Link instead of failing to build.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

#[cfg_attr(not(target_arch = "wasm32"), path = "os.rs")]
#[cfg_attr(target_arch = "wasm32", path = "none.rs")]
mod imp;

/// One IPv4 address of one interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
    /// The broadcast address the interface reports, if any (an address
    /// added without `brd` reports none, or its own address).
    pub broadcast: Option<Ipv4Addr>,
    pub loopback: bool,
}

/// This host's IPv4 addresses, every interface.  Cheap enough to call on
/// every Link announce.
pub fn interfaces() -> Vec<Iface> { imp::interfaces() }

/// Every address of this host, IPv4 and IPv6 (to recognise our own
/// services when browsing for others).
pub fn local_ips() -> Vec<IpAddr> { imp::local_ips() }

/// An interface's hardware address, where the OS tells it (Linux).
pub fn mac(iface: &str) -> Option<[u8; 6]> { imp::mac(iface) }

/// A UDP socket on `0.0.0.0:port` that shares the port with other Pro DJ
/// Link tools (prolink_virtual_cdj, dysentery, a second freedj): reuse
/// address and port, broadcast on.
pub fn bind_shared_udp(port: u16) -> std::io::Result<UdpSocket> { imp::bind_shared_udp(port) }

/// A service found on the local network by mDNS.
#[derive(Clone, Debug)]
pub struct Found {
    pub addrs: Vec<IpAddr>,
    pub port: u16,
    pub fullname: String,
}

/// Browse mDNS for `service` (e.g. `_stemd._tcp.local.`), calling `found`
/// for each instance resolved.  Blocks for as long as the browse runs:
/// call it from a thread of its own.  Returns at once where there is no
/// mDNS.
pub fn mdns_browse(service: &str, found: impl FnMut(Found)) { imp::mdns_browse(service, found) }

/// Advertise `service` on mDNS as `instance` on `port` with TXT `props`,
/// for as long as the returned [`Advert`] lives.
pub fn mdns_advertise(service: &str, instance: &str, port: u16, props: &[(&str, &str)]) -> std::io::Result<Advert> {
    imp::mdns_advertise(service, instance, port, props).map(Advert)
}

/// An mDNS advertisement, withdrawn when dropped.
pub struct Advert(#[allow(dead_code)] imp::Advert);

/// This host's name as an mDNS label: `$HOSTNAME` with anything but
/// letters, digits and `-` made `-`, or "opendeck".
pub fn host_label() -> String {
    let host = std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| "opendeck".into());
    host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect()
}

/// The socket address helper every caller wants: `0.0.0.0:port`.
pub fn any_v4(port: u16) -> SocketAddr { SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)) }
