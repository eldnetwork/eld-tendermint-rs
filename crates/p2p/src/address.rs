//! `p2p.NetAddress`. `ID@host:port`, optional `tcp://` prefix.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use eld_tendermint_proto::p2p::NetAddress as ProtoNetAddress;

use crate::Error;

/// Peer id, IP, and port. The id is 40 lowercase hex characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetAddress {
    pub id: String,
    pub ip: IpAddr,
    pub port: u16,
}

impl NetAddress {
    /// `NewNetAddressString`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNetAddress`] when the id, host, or port is missing or invalid.
    pub fn parse(input: &str) -> Result<Self, Error> {
        let raw = strip_tcp(input.trim());
        let Some((id, hostport)) = raw.split_once('@') else {
            return Err(invalid(input));
        };
        if id.len() != 40 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid(input));
        }
        let socket = resolve_hostport(hostport).map_err(|_| invalid(input))?;
        Ok(Self {
            id: id.to_ascii_lowercase(),
            ip: socket.ip(),
            port: socket.port(),
        })
    }

    /// Host and port passed to `TcpStream::connect`.
    #[must_use]
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.port)
    }

    #[must_use]
    pub fn to_proto(&self) -> ProtoNetAddress {
        ProtoNetAddress {
            id: self.id.clone(),
            ip: self.ip.to_string(),
            port: u32::from(self.port),
        }
    }

    /// # Errors
    ///
    /// Returns [`Error::InvalidNetAddress`] when the protobuf ip or port is invalid.
    pub fn from_proto(proto: &ProtoNetAddress) -> Result<Self, Error> {
        if proto.id.len() != 40 || !proto.id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid(&proto.id));
        }
        let port = u16::try_from(proto.port).map_err(|_| invalid(&proto.ip))?;
        let ip = proto.ip.parse::<IpAddr>().map_err(|_| invalid(&proto.ip))?;
        Ok(Self {
            id: proto.id.to_ascii_lowercase(),
            ip,
            port,
        })
    }

    /// Go `net.IP` JSON: standard base64 of the raw bytes. IPv4 is the 16-byte
    /// IPv4-mapped form from `net.ParseIP`.
    #[must_use]
    pub fn ip_json(&self) -> String {
        STANDARD.encode(go_ip_bytes(self.ip))
    }

    /// # Errors
    ///
    /// Returns [`Error::InvalidNetAddress`] when `ip` is not base64 of 4 or 16 address bytes.
    pub fn ip_from_json(ip: &str) -> Result<IpAddr, Error> {
        let bytes = STANDARD.decode(ip).map_err(|_| invalid(ip))?;
        ip_from_go_bytes(&bytes).ok_or_else(|| invalid(ip))
    }
}

/// Comma-separated `persistent_peers`. Empty entries are skipped.
///
/// # Errors
///
/// Returns [`Error::InvalidNetAddress`] on the first entry that does not parse.
pub fn parse_persistent_peers(peers: &str) -> Result<Vec<NetAddress>, Error> {
    let mut out = Vec::new();
    for entry in peers.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        out.push(NetAddress::parse(entry)?);
    }
    Ok(out)
}

fn strip_tcp(input: &str) -> &str {
    input.strip_prefix("tcp://").unwrap_or(input)
}

fn resolve_hostport(hostport: &str) -> Result<SocketAddr, ()> {
    if let Ok(addr) = hostport.parse::<SocketAddr>() {
        return Ok(addr);
    }
    hostport.to_socket_addrs().map_err(|_| ())?.next().ok_or(())
}

fn go_ip_bytes(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            let mut bytes = vec![0u8; 10];
            bytes.extend_from_slice(&[0xff, 0xff]);
            bytes.extend_from_slice(&octets);
            bytes
        }
        IpAddr::V6(v6) => v6.octets().to_vec(),
    }
}

fn ip_from_go_bytes(bytes: &[u8]) -> Option<IpAddr> {
    match bytes.len() {
        4 => Some(IpAddr::V4(Ipv4Addr::new(
            bytes[0], bytes[1], bytes[2], bytes[3],
        ))),
        16 => {
            let mapped =
                bytes[..10].iter().all(|byte| *byte == 0) && bytes[10] == 0xff && bytes[11] == 0xff;
            if mapped {
                Some(IpAddr::V4(Ipv4Addr::new(
                    bytes[12], bytes[13], bytes[14], bytes[15],
                )))
            } else {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(bytes);
                Some(IpAddr::V6(Ipv6Addr::from(octets)))
            }
        }
        _ => None,
    }
}

fn invalid(addr: &str) -> Error {
    Error::InvalidNetAddress {
        addr: addr.to_owned(),
    }
}
