//! Who is on the other end of a connection.
//!
//! Studio's loopback port can be reached by every account on the computer,
//! and the page it serves holds the write token. So it answers only the
//! account that started it, and root, which could read its files anyway.
//!
//! The kernel lists the owner of each TCP socket in `/proc/net/tcp`. The
//! client's socket is the row whose local end is this connection's remote
//! end. Anything that cannot be identified is refused, and that includes a
//! socket the kernel has orphaned: it lists those as owned by root, with
//! inode 0, whoever opened them.

use std::net::{IpAddr, SocketAddr, TcpStream};
use std::os::unix::fs::MetadataExt;

pub fn allowed(stream: &TcpStream) -> bool {
    let (Ok(remote), Ok(local)) = (stream.peer_addr(), stream.local_addr()) else {
        return false;
    };
    let Ok(own) = std::fs::metadata("/proc/self").map(|meta| meta.uid()) else {
        return false;
    };
    let table = if remote.is_ipv4() {
        "/proc/net/tcp"
    } else {
        "/proc/net/tcp6"
    };
    let Ok(rows) = std::fs::read_to_string(table) else {
        return false;
    };
    owner_of(&rows, remote, local).is_some_and(|uid| uid == own || uid == 0)
}

/// The kernel prints each 32-bit word of an address in host byte order.
fn proc_address(address: SocketAddr) -> String {
    let words: Vec<String> = match address.ip() {
        IpAddr::V4(ip) => vec![format!("{:08X}", u32::from_ne_bytes(ip.octets()))],
        IpAddr::V6(ip) => ip
            .octets()
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| format!("{:08X}", u32::from_ne_bytes(*word)))
            .collect(),
    };
    format!("{}:{:04X}", words.concat(), address.port())
}

/// The uid of the socket that runs from `client` to `server`.
fn owner_of(rows: &str, client: SocketAddr, server: SocketAddr) -> Option<u32> {
    let (client, server) = (proc_address(client), proc_address(server));
    rows.lines().skip(1).find_map(|row| {
        let columns: Vec<&str> = row.split_whitespace().collect();
        let (local, remote, uid) = (columns.get(1)?, columns.get(2)?, columns.get(7)?);
        let inode = columns.get(9)?;
        let uid: u32 = uid.parse().ok()?;
        (local.eq_ignore_ascii_case(&client)
            && remote.eq_ignore_ascii_case(&server)
            && !(uid == 0 && *inode == "0"))
            .then_some(uid)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const ROWS: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:4D31 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11 1
   1: 0100007F:4D31 0100007F:C000 01 00000000:00000000 00:00000000 00000000  1000        0 12 1
   2: 0100007F:C000 0100007F:4D31 01 00000000:00000000 00:00000000 00000000  1234        0 13 1
   3: 0100007F:C001 0100007F:4D31 05 00000000:00000000 00:00000000 00000000     0        0 0 1
   4: 0100007F:C002 0100007F:4D31 04 00000000:00000000 00:00000000 00000000  1234        0 0 1
";

    #[test]
    fn finds_the_client_end_of_the_connection() {
        let client: SocketAddr = "127.0.0.1:49152".parse().unwrap();
        let server: SocketAddr = "127.0.0.1:19761".parse().unwrap();
        // The server's own row (uid 1000) must not be taken for the client's.
        assert_eq!(owner_of(ROWS, client, server), Some(1234));
        let other: SocketAddr = "127.0.0.1:49155".parse().unwrap();
        assert_eq!(owner_of(ROWS, other, server), None);
    }

    #[test]
    fn an_orphaned_socket_does_not_count_as_root() {
        let orphan: SocketAddr = "127.0.0.1:49153".parse().unwrap();
        let server: SocketAddr = "127.0.0.1:19761".parse().unwrap();
        assert_eq!(owner_of(ROWS, orphan, server), None);
        // One that is still closing keeps its owner, so a script that
        // writes and hangs up is still recognised.
        let closing: SocketAddr = "127.0.0.1:49154".parse().unwrap();
        assert_eq!(owner_of(ROWS, closing, server), Some(1234));
    }

    #[test]
    fn prints_addresses_like_the_kernel() {
        let v4: SocketAddr = "127.0.0.1:19777".parse().unwrap();
        assert_eq!(
            proc_address(v4),
            format!("{:08X}:4D41", u32::from_ne_bytes([127, 0, 0, 1]))
        );
        let v6: SocketAddr = "[::1]:80".parse().unwrap();
        assert_eq!(
            proc_address(v6),
            format!(
                "000000000000000000000000{:08X}:0050",
                u32::from_ne_bytes([0, 0, 0, 1])
            )
        );
    }

    #[test]
    fn accepts_a_connection_from_this_account() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        assert!(allowed(&stream));
    }
}
