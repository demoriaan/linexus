//! ICMP echo on ping sockets and the TCP fallback: one probe each.

use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::os::fd::FromRawFd;
use std::time::{Duration, Instant};

/// Port the TCP fallback connects to.
const TCP_PORT: u16 = 443;
pub(super) const PAYLOAD: &[u8; 8] = b"linexus\0";

/// What became of one probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    Answered(Duration),
    /// No answer within the timeout.
    Lost,
    /// The socket could not send or receive (no route, the source address
    /// went away); a fresh socket may work where this one does not.
    Failed,
}

/// An echo request with sequence number `seq`; identifier and checksum are
/// left at zero for the kernel to fill in.
pub(super) fn echo_request(v6: bool, seq: u16) -> [u8; 16] {
    let mut packet = [0u8; 16];
    packet[0] = if v6 { 128 } else { 8 };
    packet[6..8].copy_from_slice(&seq.to_be_bytes());
    packet[8..].copy_from_slice(PAYLOAD);
    packet
}

/// The sequence number of an echo reply as a ping socket delivers it.
pub(super) fn echo_reply_seq(v6: bool, packet: &[u8]) -> Option<u16> {
    let reply_type = if v6 { 129 } else { 0 };
    match packet {
        [kind, 0, _, _, _, _, hi, lo, ..] if *kind == reply_type => {
            Some(u16::from_be_bytes([*hi, *lo]))
        }
        _ => None,
    }
}

/// Opens a ping socket for IPv6 or IPv4. It stays unconnected: connect()
/// would pin the source address, and after a network change (new DHCP
/// lease, another Wi-Fi, a VPN) every send would fail.
pub(super) fn open_icmp(v6: bool) -> io::Result<UdpSocket> {
    let (domain, protocol) = if v6 {
        (libc::AF_INET6, libc::IPPROTO_ICMPV6)
    } else {
        (libc::AF_INET, libc::IPPROTO_ICMP)
    };
    // SAFETY: plain socket(2) call; the descriptor is checked before use.
    let fd = unsafe { libc::socket(domain, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, protocol) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a datagram socket that nothing else owns. UdpSocket
    // only issues sendto, recv and setsockopt, which ping sockets support
    // like UDP ones.
    Ok(unsafe { UdpSocket::from_raw_fd(fd) })
}

/// socket(2) refused because the system keeps this user out of ping
/// sockets (`ping_group_range`).
pub(super) fn icmp_denied(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EACCES | libc::EPERM))
}

/// One echo round trip to `addr`.
pub(super) fn icmp_round_trip(
    socket: &UdpSocket,
    addr: IpAddr,
    seq: u16,
    timeout: Duration,
) -> Outcome {
    let v6 = addr.is_ipv6();
    let start = Instant::now();
    if socket
        .send_to(&echo_request(v6, seq), SocketAddr::new(addr, 0))
        .is_err()
    {
        return Outcome::Failed;
    }
    let mut buffer = [0u8; 512];
    loop {
        let Some(left) = timeout
            .checked_sub(start.elapsed())
            .filter(|left| !left.is_zero())
        else {
            return Outcome::Lost;
        };
        if socket.set_read_timeout(Some(left)).is_err() {
            return Outcome::Failed;
        }
        match socket.recv(&mut buffer) {
            // Late replies to earlier probes carry older numbers.
            Ok(n) if echo_reply_seq(v6, &buffer[..n]) == Some(seq) => {
                return Outcome::Answered(start.elapsed())
            }
            Ok(_) => {}
            Err(error) => match error.kind() {
                io::ErrorKind::Interrupted => {}
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => return Outcome::Lost,
                _ => return Outcome::Failed,
            },
        }
    }
}

/// The time a TCP connection to port 443 takes. A refused connection came
/// back from the host too, so it counts.
pub(super) fn tcp_round_trip(addr: IpAddr, timeout: Duration) -> Outcome {
    let start = Instant::now();
    match TcpStream::connect_timeout(&SocketAddr::new(addr, TCP_PORT), timeout) {
        Ok(_) => Outcome::Answered(start.elapsed()),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            Outcome::Answered(start.elapsed())
        }
        Err(_) => Outcome::Lost,
    }
}

pub(super) enum Prober {
    /// A ping socket of the family of the address it was opened for.
    Icmp {
        socket: UdpSocket,
        v6: bool,
    },
    Tcp,
}
