//! No OS network (a browser): no interfaces, and every socket refuses.

use crate::{Found, Iface};
use std::io::{Error, ErrorKind};
use std::net::{IpAddr, UdpSocket};

fn unsupported() -> Error { Error::new(ErrorKind::Unsupported, "no network on this platform") }

pub fn interfaces() -> Vec<Iface> { Vec::new() }

pub fn local_ips() -> Vec<IpAddr> { Vec::new() }

pub fn mac(_iface: &str) -> Option<[u8; 6]> { None }

pub fn bind_shared_udp(_port: u16) -> std::io::Result<UdpSocket> { Err(unsupported()) }

pub fn mdns_browse(_service: &str, _found: impl FnMut(Found)) {}

pub struct Advert;

pub fn mdns_advertise(_service: &str, _instance: &str, _port: u16, _props: &[(&str, &str)]) -> std::io::Result<Advert> {
    Err(unsupported())
}
