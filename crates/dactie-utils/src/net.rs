use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;

/// GossipSub mesh band of every node. GossipSub grafts new mesh peers only below MESH_N_LOW
/// and prunes only above MESH_N_HIGH, so any degree in between is stable, and the paper's
/// anonymity bound is evaluated at MESH_N_HIGH. The anonymity experiments use the same band.
pub const MESH_N_LOW: usize = 10;
pub const MESH_N: usize = 12;
pub const MESH_N_HIGH: usize = 24;

/// Returns true for addresses that must not be added to the DHT because no remote
/// host can dial them.
///
/// Peers run with `network_mode: host`, so `identify` announces every interface on the
/// machine, including the Docker bridges (`172.17.0.1`, `172.18.0.1`, ...). Adding those
/// makes every other node dial them and log `Connection refused` forever.
///
/// Note: this deliberately does NOT filter all private ranges. The deployment network is
/// `10.244.192.0/24`, so `Ipv4Addr::is_private()` would discard the real addresses. Only
/// loopback, link-local and the `172.16.0.0/12` bridge range are rejected.
pub fn is_unroutable_addr(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| match p {
        Protocol::Ip4(ip) => {
            let o = ip.octets();
            ip.is_loopback() || ip.is_link_local() || (o[0] == 172 && (16..=31).contains(&o[1]))
        }
        Protocol::Ip6(ip) => ip.is_loopback(),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn addr(s: &str) -> Multiaddr {
        Multiaddr::from_str(s).unwrap()
    }

    #[test]
    fn rejects_loopback_and_docker_bridges() {
        assert!(is_unroutable_addr(&addr("/ip4/127.0.0.1/tcp/10001")));
        assert!(is_unroutable_addr(&addr("/ip4/172.17.0.1/tcp/20012")));
        assert!(is_unroutable_addr(&addr("/ip4/172.18.0.1/tcp/20012")));
        assert!(is_unroutable_addr(&addr("/ip4/172.31.255.254/tcp/20012")));
    }

    #[test]
    fn keeps_the_deployment_network() {
        assert!(!is_unroutable_addr(&addr("/ip4/10.244.192.116/tcp/10001")));
        assert!(!is_unroutable_addr(&addr("/ip4/10.244.192.117/tcp/20011")));
        assert!(!is_unroutable_addr(&addr("/ip4/172.15.0.1/tcp/20011")));
        assert!(!is_unroutable_addr(&addr("/ip4/172.32.0.1/tcp/20011")));
    }
}
