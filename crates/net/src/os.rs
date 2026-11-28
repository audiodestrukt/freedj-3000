//! A native OS's network: if-addrs, socket2, mdns-sd.

use crate::{any_v4, Found, Iface};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, UdpSocket};

pub fn interfaces() -> Vec<Iface> {
    if_addrs::get_if_addrs().unwrap_or_default().into_iter().filter_map(|i| {
        let loopback = i.is_loopback();
        match i.addr {
            if_addrs::IfAddr::V4(v4) => Some(Iface { name: i.name, ip: v4.ip, netmask: v4.netmask, broadcast: v4.broadcast, loopback }),
            _ => None,
        }
    }).collect()
}

pub fn local_ips() -> Vec<IpAddr> {
    if_addrs::get_if_addrs().map(|v| v.into_iter().map(|i| i.ip()).collect()).unwrap_or_default()
}

pub fn mac(iface: &str) -> Option<[u8; 6]> {
    let m = std::fs::read_to_string(format!("/sys/class/net/{iface}/address")).ok()?;
    let b: Vec<u8> = m.trim().split(':').filter_map(|h| u8::from_str_radix(h, 16).ok()).collect();
    b.try_into().ok()
}

pub fn bind_shared_udp(port: u16) -> std::io::Result<UdpSocket> {
    let raw = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    raw.set_reuse_address(true).ok();
    #[cfg(unix)]
    raw.set_reuse_port(true).ok();
    raw.set_broadcast(true).ok();
    raw.bind(&any_v4(port).into())?;
    Ok(raw.into())
}

pub fn mdns_browse(service: &str, mut found: impl FnMut(Found)) {
    let Ok(d) = mdns_sd::ServiceDaemon::new() else { return };
    let Ok(rx) = d.browse(service) else { return };
    while let Ok(ev) = rx.recv() {
        if let mdns_sd::ServiceEvent::ServiceResolved(info) = ev {
            found(Found {
                addrs: info.get_addresses().iter().map(|a| a.to_ip_addr()).collect(),
                port: info.get_port(),
                fullname: info.get_fullname().to_string(),
            });
        }
    }
}

pub type Advert = mdns_sd::ServiceDaemon;

pub fn mdns_advertise(service: &str, instance: &str, port: u16, props: &[(&str, &str)]) -> std::io::Result<Advert> {
    let err = |e: mdns_sd::Error| std::io::Error::other(e.to_string());
    let d = mdns_sd::ServiceDaemon::new().map_err(err)?;
    let info = mdns_sd::ServiceInfo::new(service, instance, &format!("{}.local.", crate::host_label()), "", port, props)
        .map_err(err)?
        .enable_addr_auto();
    d.register(info).map_err(err)?;
    Ok(d)
}
