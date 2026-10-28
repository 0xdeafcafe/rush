//! What netwatch reads from the machine: each interface's byte counters
//! (Go internal/netwatch/counters_darwin.go, counters_linux.go,
//! counters_other.go), which network it's on (netwatch.go:441 network), and
//! a wakeup when its addresses or links change, so a new network is
//! noticed at once without polling: a routing socket on macOS, a netlink
//! socket on Linux. Go noticed it on its 5s sample instead.

use std::collections::HashMap;
use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Each interface's bytes in and out, by index.
pub(crate) type Counters = HashMap<u32, [u64; 2]>;

/// tunnel is an interface that carries other interfaces' traffic, or none
/// of the machine's own: VPNs, AirDrop, bridges for VMs.
pub(crate) fn tunnel(name: &str) -> bool {
    [
        "utun", "ipsec", "gif", "stf", "awdl", "llw", "bridge", "anpi", "ap", "tun", "tap",
        "docker", "veth", "br-", "virbr", "vmnet", "lo",
    ]
    .iter()
    .any(|p| name.starts_with(p))
}

/// Go's IP.IsGlobalUnicast: not unspecified, loopback, multicast,
/// link-local or the IPv4 broadcast address. Private addresses count.
pub(crate) fn global_unicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            v != Ipv4Addr::BROADCAST
                && !v.is_unspecified()
                && !v.is_loopback()
                && !v.is_multicast()
                && !v.is_link_local()
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return global_unicast(IpAddr::V4(v4));
            }
            v != Ipv6Addr::UNSPECIFIED
                && !v.is_loopback()
                && !v.is_multicast()
                && (v.segments()[0] & 0xffc0) != 0xfe80
        }
    }
}

/// Which network the machine is on: its interfaces that are up with an
/// address to the outside, as a key, and the first IPv4 one as a label.
pub(crate) fn network() -> (String, String) {
    let mut found: Vec<(String, IpAddr)> = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills ifap with a list freed below; every pointer
    // read is checked for null first.
    unsafe {
        if libc::getifaddrs(&mut ifap) != 0 {
            return (String::new(), String::new());
        }
        let mut p = ifap;
        while !p.is_null() {
            let ifa = &*p;
            p = ifa.ifa_next;
            let flags = ifa.ifa_flags as libc::c_int;
            if flags & libc::IFF_UP == 0
                || flags & libc::IFF_LOOPBACK != 0
                || ifa.ifa_addr.is_null()
            {
                continue;
            }
            let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
            if tunnel(&name) {
                continue;
            }
            let ip = match (*ifa.ifa_addr).sa_family as libc::c_int {
                libc::AF_INET => {
                    let sa = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    IpAddr::V4(Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr)))
                }
                libc::AF_INET6 => {
                    let sa = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                    IpAddr::V6(Ipv6Addr::from(sa.sin6_addr.s6_addr))
                }
                _ => continue,
            };
            found.push((name, ip));
        }
        libc::freeifaddrs(ifap);
    }
    network_of(&found)
}

/// network's key and label from the interfaces' addresses, in the order
/// the system lists them.
pub(crate) fn network_of(addrs: &[(String, IpAddr)]) -> (String, String) {
    let mut parts = Vec::new();
    let mut label = String::new();
    for (name, ip) in addrs {
        if !global_unicast(*ip) {
            continue;
        }
        let ip = match ip {
            IpAddr::V6(v) => v.to_ipv4_mapped().map_or(*ip, IpAddr::V4),
            _ => *ip,
        };
        let part = format!("{name} {ip}");
        if label.is_empty() && ip.is_ipv4() {
            label = part.clone();
        }
        parts.push(part);
    }
    parts.sort();
    if label.is_empty()
        && let Some(p) = parts.first()
    {
        label = p.clone();
    }
    if parts.is_empty() {
        return ("none".into(), "no network".into());
    }
    (parts.join(","), label)
}

/// Each interface's bytes in and out, by its place in /proc/net/dev.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_proc_net_dev(text: &str) -> Counters {
    let mut out = Counters::new();
    for (i, l) in text.split('\n').enumerate() {
        let Some((name, rest)) = l.split_once(':') else {
            continue;
        };
        if tunnel(name.trim()) {
            continue;
        }
        let f: Vec<&str> = rest.split_whitespace().collect();
        if f.len() < 9 {
            continue;
        }
        let r = f[0].parse().unwrap_or(0);
        let t = f[8].parse().unwrap_or(0);
        out.insert(i as u32 + 1, [r, t]);
    }
    out
}

#[cfg(target_os = "linux")]
pub(crate) fn counters() -> Option<Counters> {
    std::fs::read_to_string("/proc/net/dev")
        .ok()
        .map(|t| parse_proc_net_dev(&t))
}

/// The bytes in and out of each of the machine's own interfaces, by index,
/// read from the routing table's interface list: no process run. macOS
/// hands them to a process that isn't root cut to 32 bits, so they wrap
/// every 4G; rates allow for that.
#[cfg(target_os = "macos")]
pub(crate) fn counters() -> Option<Counters> {
    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
    let mut len: libc::size_t = 0;
    // SAFETY: sysctl is asked for the size first, then filled into a
    // buffer of that size; messages are read only within what it wrote.
    unsafe {
        let null = std::ptr::null_mut();
        if libc::sysctl(mib.as_mut_ptr(), 6, null, &mut len, null, 0) != 0 {
            return None;
        }
        let mut buf = vec![0u8; len + len / 8];
        len = buf.len();
        if libc::sysctl(
            mib.as_mut_ptr(),
            6,
            buf.as_mut_ptr().cast(),
            &mut len,
            null,
            0,
        ) != 0
        {
            return None;
        }
        buf.truncate(len);
        let mut out = Counters::new();
        let mut b = &buf[..];
        while b.len() >= 4 {
            let n = u16::from_ne_bytes([b[0], b[1]]) as usize;
            if n == 0 || n > b.len() {
                break;
            }
            if b[3] as libc::c_int == libc::RTM_IFINFO2
                && n >= std::mem::size_of::<libc::if_msghdr2>()
            {
                let m: libc::if_msghdr2 = std::ptr::read_unaligned(b.as_ptr().cast());
                // Ethernet and Wi-Fi (6), and cellular (0xff): not
                // loopback, tunnels or bridges, whose bytes are counted
                // where they leave.
                if m.ifm_flags & libc::IFF_UP != 0
                    && (m.ifm_data.ifi_type == 6 || m.ifm_data.ifi_type == 0xff)
                {
                    out.insert(
                        m.ifm_index as u32,
                        [m.ifm_data.ifi_ibytes, m.ifm_data.ifi_obytes],
                    );
                }
            }
            b = &b[n..];
        }
        Some(out)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn counters() -> Option<Counters> {
    None
}

/// Calls changed(true) each time the machine's addresses or links change,
/// from a thread that sleeps in the kernel between, and changed(false) if
/// it stops being able to hear of them; false where there's no way to hear
/// of it at all (the caller then looks on its sample instead). The thread
/// ends when changed returns false.
#[cfg(target_os = "macos")]
pub(crate) fn on_change(changed: impl FnMut(bool) -> bool + Send + 'static) -> bool {
    // SAFETY: a plain socket, kept from the processes rush starts (macOS
    // has no SOCK_CLOEXEC); closed when the thread ends.
    let fd = unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW, libc::AF_UNSPEC) };
    if fd < 0 {
        return false;
    }
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    listen(fd, changed, |m| {
        // Only addresses and links: routes and ARP entries come and go all
        // the time on a busy network.
        m.len() >= 4
            && matches!(
                m[3] as libc::c_int,
                libc::RTM_NEWADDR | libc::RTM_DELADDR | libc::RTM_IFINFO
            )
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn on_change(changed: impl FnMut(bool) -> bool + Send + 'static) -> bool {
    // SAFETY: a netlink socket bound to the link and address groups.
    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if fd < 0 {
        return false;
    }
    let ok = unsafe {
        let mut sa: libc::sockaddr_nl = std::mem::zeroed();
        sa.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        sa.nl_groups =
            (libc::RTMGRP_LINK | libc::RTMGRP_IPV4_IFADDR | libc::RTMGRP_IPV6_IFADDR) as u32;
        libc::bind(
            fd,
            (&sa as *const libc::sockaddr_nl).cast(),
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        ) == 0
    };
    if !ok {
        unsafe { libc::close(fd) };
        return false;
    }
    listen(fd, changed, |m| !m.is_empty())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn on_change(_: impl FnMut(bool) -> bool + Send + 'static) -> bool {
    false
}

/// Reads fd's messages on a thread of its own, calling changed for each
/// that matters.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn listen(
    fd: libc::c_int,
    mut changed: impl FnMut(bool) -> bool + Send + 'static,
    matters: impl Fn(&[u8]) -> bool + Send + 'static,
) -> bool {
    let spawned = std::thread::Builder::new()
        .name("netwatch-route".into())
        .spawn(move || {
            let mut buf = vec![0u8; 8192];
            loop {
                // SAFETY: reads into buf, within its length.
                let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 {
                    match std::io::Error::last_os_error().raw_os_error() {
                        Some(libc::EINTR) => continue,
                        // The kernel had more to say than the socket held,
                        // and dropped some: what was dropped may have been
                        // a change, so look.
                        Some(libc::ENOBUFS) => {
                            if !changed(true) {
                                break;
                            }
                            continue;
                        }
                        _ => {
                            changed(false);
                            break;
                        }
                    }
                }
                if n == 0 {
                    changed(false);
                    break;
                }
                if matters(&buf[..n as usize]) && !changed(true) {
                    break;
                }
            }
            unsafe { libc::close(fd) };
        });
    if spawned.is_err() {
        unsafe { libc::close(fd) };
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnels() {
        for (name, want) in [
            ("en0", false),
            ("eth0", false),
            ("wlan0", false),
            ("utun3", true),
            ("awdl0", true),
            ("bridge100", true),
            ("lo0", true),
            ("docker0", true),
            ("br-1234", true),
        ] {
            assert_eq!(tunnel(name), want, "{name}");
        }
    }

    #[test]
    fn network_key_and_label() {
        let a = |n: &str, ip: &str| (n.to_string(), ip.parse::<IpAddr>().unwrap());
        type Case = (Vec<(String, IpAddr)>, &'static str, &'static str);
        let cases: Vec<Case> = vec![
            (vec![], "none", "no network"),
            (
                vec![a("en0", "fe80::1"), a("en0", "169.254.3.4")],
                "none",
                "no network",
            ),
            (
                vec![
                    a("en0", "2a00::5"),
                    a("en0", "192.168.1.20"),
                    a("en1", "10.0.0.2"),
                ],
                "en0 192.168.1.20,en0 2a00::5,en1 10.0.0.2",
                "en0 192.168.1.20",
            ),
            (vec![a("en0", "2a00::5")], "en0 2a00::5", "en0 2a00::5"),
        ];
        for (addrs, key, label) in cases {
            assert_eq!(network_of(&addrs), (key.to_string(), label.to_string()));
        }
    }

    #[test]
    fn proc_net_dev() {
        let text = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 100 1 0 0 0 0 0 0 100 1 0 0 0 0 0 0
  eth0: 5000 40 0 0 0 0 0 0 7000 50 0 0 0 0 0 0
docker0: 9 1 0 0 0 0 0 0 9 1 0 0 0 0 0 0
";
        let c = parse_proc_net_dev(text);
        assert_eq!(c, Counters::from([(4, [5000, 7000])]));
    }
}
