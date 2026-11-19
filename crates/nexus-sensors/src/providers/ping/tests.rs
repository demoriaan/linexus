use super::probe::*;
use super::*;
use std::io;
use std::net::{Ipv6Addr, SocketAddr, UdpSocket};

fn close(a: Option<f32>, b: f32) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-4)
}

#[test]
fn stats_over_answered_and_lost_probes() {
    let history = [Some(10.0), None, Some(14.0), Some(12.0), None];
    let stats = stats(&history);
    assert_eq!(stats.last_ms, Some(12.0));
    assert!(close(stats.avg_ms, 12.0));
    assert_eq!(stats.min_ms, Some(10.0));
    assert_eq!(stats.max_ms, Some(14.0));
    // |14 - 10| and |12 - 14| between consecutive answers, lost ones skipped.
    assert!(close(stats.jitter_ms, 3.0));
    assert!(close(stats.loss_percent, 40.0));
    assert_eq!(stats.lost_in_row, 1);
    assert!(!stats.down());
}

#[test]
fn stats_before_and_without_answers() {
    assert_eq!(stats(&[]), PingStats::default());
    let one = stats(&[Some(7.5)]);
    assert_eq!((one.last_ms, one.jitter_ms), (Some(7.5), None));
    assert!(close(one.loss_percent, 0.0));
    let silent = stats(&[Some(9.0), None, None, None]);
    assert_eq!(silent.lost_in_row, 3);
    assert!(silent.down());
    assert_eq!(silent.last_ms, Some(9.0), "the last answer stays known");
    assert!(close(silent.loss_percent, 75.0));
}

#[test]
fn the_window_keeps_the_last_sixty_probes() {
    let mut probes = VecDeque::new();
    for i in 0..100 {
        if probes.len() == WINDOW {
            probes.pop_front();
        }
        probes.push_back((i % 10 != 0).then_some(i as f32));
    }
    let history: Vec<Option<f32>> = probes.into_iter().collect();
    assert_eq!(history.len(), WINDOW);
    assert_eq!(history[..2], [None, Some(41.0)]);
    // 40, 50, 60, 70, 80 and 90 were lost: 6 of 60.
    assert!(close(stats(&history).loss_percent, 10.0));
}

#[test]
fn echo_packets() {
    let v4 = echo_request(false, 0x1234);
    assert_eq!(&v4[..8], &[8, 0, 0, 0, 0, 0, 0x12, 0x34]);
    assert_eq!(&v4[8..], PAYLOAD);
    assert_eq!(echo_request(true, 1)[0], 128);

    let mut reply = v4;
    reply[0] = 0;
    reply[4..6].copy_from_slice(&[0x6b, 0x5c]);
    assert_eq!(echo_reply_seq(false, &reply), Some(0x1234));
    assert_eq!(echo_reply_seq(true, &reply), None, "not an ICMPv6 reply");
    assert_eq!(echo_reply_seq(false, &v4), None, "a request is no reply");
    assert_eq!(echo_reply_seq(false, &reply[..7]), None, "too short");
    reply[1] = 1;
    assert_eq!(echo_reply_seq(false, &reply), None, "wrong code");
    assert_eq!(echo_reply_seq(true, &[129, 0, 9, 9, 0, 1, 0, 7]), Some(7));
}

#[test]
fn default_gateway_from_the_route_table() {
    // The kernel prints each address's bytes read as a number in host order.
    let hex = |bytes: [u8; 4]| format!("{:08X}", u32::from_ne_bytes(bytes));
    let (lan, vpn, net) = (
        hex([192, 168, 0, 1]),
        hex([10, 8, 8, 1]),
        hex([192, 168, 0, 0]),
    );
    let table = format!(
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
        enp11s0\t00000000\t{lan}\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
        tun0\t00000000\t{vpn}\t0003\t0\t0\t50\t00000000\t0\t0\t0\n\
        down0\t00000000\t01010101\t0002\t0\t0\t1\t00000000\t0\t0\t0\n\
        enp11s0\t{net}\t00000000\t0001\t0\t0\t100\t{mask}\t0\t0\t0\n",
        mask = hex([255, 255, 255, 0])
    );
    assert_eq!(
        parse_default_gateway(&table),
        Some(Ipv4Addr::new(10, 8, 8, 1))
    );
    let lan_only = table
        .lines()
        .filter(|l| !l.starts_with("tun0"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        parse_default_gateway(&lan_only),
        Some(Ipv4Addr::new(192, 168, 0, 1))
    );
    assert_eq!(parse_default_gateway("Iface\tDestination\n"), None);
}

#[test]
fn probe_timeouts_leave_room_for_red_round_trips() {
    assert_eq!(probe_timeout(None), PROBE_TIMEOUT);
    assert_eq!(probe_timeout(Some(120)), PROBE_TIMEOUT);
    assert_eq!(probe_timeout(Some(2000)), Duration::from_millis(2500));
    assert_eq!(probe_timeout(Some(10_000)), Duration::from_millis(10_500));
}

#[test]
fn addresses_a_probe_can_reach() {
    let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
    assert_eq!(probe_address(mapped), Some(IpAddr::from([192, 0, 2, 7])));
    assert_eq!(probe_address("fe80::1".parse().unwrap()), None);
    let global: IpAddr = "2001:db8::1".parse().unwrap();
    assert_eq!(probe_address(global), Some(global));
}

#[test]
fn only_a_refused_ping_socket_falls_back_to_tcp() {
    assert!(icmp_denied(&io::Error::from_raw_os_error(libc::EACCES)));
    assert!(icmp_denied(&io::Error::from_raw_os_error(libc::EPERM)));
    assert!(!icmp_denied(&io::Error::from_raw_os_error(
        libc::EAFNOSUPPORT
    )));
}

/// A ping socket for loopback, or `None` (the test then passes without
/// checking) where this user may not open one.
fn loopback(addr: IpAddr) -> Option<UdpSocket> {
    match open_icmp(addr.is_ipv6()) {
        Ok(socket) => Some(socket),
        Err(error) if icmp_denied(&error) || error.raw_os_error() == Some(libc::EAFNOSUPPORT) => {
            eprintln!("skipped: no ping socket for {addr}: {error}");
            None
        }
        Err(error) => panic!("ping socket for {addr}: {error}"),
    }
}

#[test]
fn loopback_answers_echo_requests() {
    for addr in [
        IpAddr::from([127, 0, 0, 1]),
        IpAddr::from(Ipv6Addr::LOCALHOST),
    ] {
        let Some(socket) = loopback(addr) else {
            continue;
        };
        for seq in [1, 2, 0xffff] {
            let outcome = icmp_round_trip(&socket, addr, seq, PROBE_TIMEOUT);
            assert!(
                matches!(outcome, Outcome::Answered(rtt) if rtt < PROBE_TIMEOUT),
                "{addr} {seq}: {outcome:?}"
            );
        }
        // The reply to 7 is never read; the probe for 8 skips it.
        let late = echo_request(addr.is_ipv6(), 7);
        socket.send_to(&late, SocketAddr::new(addr, 0)).unwrap();
        thread::sleep(Duration::from_millis(20));
        let outcome = icmp_round_trip(&socket, addr, 8, PROBE_TIMEOUT);
        assert!(
            matches!(outcome, Outcome::Answered(_)),
            "{addr}: {outcome:?}"
        );
    }
}

/// A target probing `addrs` with `prober`, as after a lookup.
fn target(addrs: &[IpAddr], prober: Option<Prober>) -> Target {
    let mut target = Target::new("test");
    target.addrs = addrs.to_vec();
    target.resolved_at = Some(Instant::now());
    target.prober = prober;
    target
}

#[test]
fn a_failing_socket_is_replaced() {
    let addr = IpAddr::from([127, 0, 0, 1]);
    if loopback(addr).is_none() {
        return;
    }
    // A plain UDP socket refuses to send to port 0, as a ping socket left
    // behind by a network change refuses to send at all.
    let broken = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut target = target(
        &[addr],
        Some(Prober::Icmp {
            socket: broken,
            v6: false,
        }),
    );
    assert_eq!(target.probe(1, PROBE_TIMEOUT), None);
    assert!(target.prober.is_none(), "the failed socket is dropped");
    assert!(
        target.probe(2, PROBE_TIMEOUT).is_some(),
        "a fresh one answers"
    );
    assert_eq!(target.lost_in_row, 0);
}

#[test]
fn a_silent_address_gives_way_to_the_next() {
    let addr = IpAddr::from([127, 0, 0, 1]);
    if loopback(addr).is_none() {
        return;
    }
    // Sending to the broadcast address fails locally (no SO_BROADCAST), so
    // nothing leaves this computer.
    let silent = IpAddr::from([255, 255, 255, 255]);
    let mut target = target(&[silent, addr], None);
    for seq in 0..SWITCH_AFTER_LOST as u16 {
        assert_eq!(target.probe(seq, PROBE_TIMEOUT), None);
    }
    assert_eq!(target.addr(), Some(addr));
    assert!(target.probe(9, PROBE_TIMEOUT).is_some());
}

#[test]
fn tcp_fallback_counts_a_refused_connection() {
    // Nothing listens on 443 here, or something does: either way loopback
    // answers at once.
    let outcome = tcp_round_trip(IpAddr::from([127, 0, 0, 1]), PROBE_TIMEOUT);
    assert!(
        matches!(outcome, Outcome::Answered(rtt) if rtt < PROBE_TIMEOUT),
        "{outcome:?}"
    );
}

fn wait_for(monitor: &PingMonitor, host: &str, done: impl Fn(&PingReading) -> bool) -> PingReading {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(reading) = monitor.reading(host).filter(&done) {
            return reading;
        }
        assert!(
            Instant::now() < deadline,
            "{host}: {:?}",
            monitor.reading(host)
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wanted(hosts: &[&str]) -> Vec<(String, Duration)> {
    hosts
        .iter()
        .map(|host| (host.to_string(), PROBE_TIMEOUT))
        .collect()
}

/// Probe threads hold the shared state; this many run.
fn threads(monitor: &PingMonitor) -> usize {
    Arc::strong_count(&monitor.shared) - 1
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_monitor_probes_wanted_hosts_only() {
    let monitor = PingMonitor::new();
    assert!(monitor.readings().is_empty());
    let hosts = ["127.0.0.1".to_string(), "localhost".to_string()];
    monitor.set_hosts(
        &wanted(&["127.0.0.1", "localhost"]),
        Duration::from_millis(250),
    );
    for host in &hosts {
        let reading = wait_for(&monitor, host, |r| r.history.len() >= 2);
        assert!(!reading.unresolved);
        assert!(reading.history.iter().all(Option::is_some), "{reading:?}");
        assert!(!reading.gateway);
    }
    assert_eq!(threads(&monitor), 2);
    monitor.set_hosts(&wanted(&["127.0.0.1"]), Duration::from_millis(250));
    assert_eq!(monitor.readings().len(), 1);
    assert_eq!(monitor.reading("localhost"), None);
    wait_until("the removed host's thread ends", || threads(&monitor) == 1);
    monitor.set_hosts(&[], Duration::from_millis(250));
    assert!(monitor.readings().is_empty());
    wait_until("every thread ends", || threads(&monitor) == 0);
}

#[test]
fn a_shorter_interval_wakes_the_waiting_thread() {
    let monitor = PingMonitor::new();
    let hosts = wanted(&["127.0.0.1"]);
    monitor.set_hosts(&hosts, Duration::from_secs(600));
    wait_for(&monitor, "127.0.0.1", |r| !r.history.is_empty());
    // Without the wake-up the next probe would wait ten minutes.
    monitor.set_hosts(&hosts, Duration::from_millis(250));
    wait_for(&monitor, "127.0.0.1", |r| r.history.len() >= 3);
    // A longer timeout reaches the running thread too.
    let slow = vec![("127.0.0.1".to_string(), Duration::from_secs(3))];
    monitor.set_hosts(&slow, Duration::from_millis(250));
    assert_eq!(threads(&monitor), 1);
    assert_eq!(
        monitor.shared.lock().hosts["127.0.0.1"].timeout,
        Duration::from_secs(3)
    );
}

#[test]
fn an_unknown_name_says_so_and_sends_nothing() {
    let monitor = PingMonitor::new();
    // .invalid never resolves (RFC 6761).
    let host = "linexus-test.invalid".to_string();
    monitor.set_hosts(&wanted(&[&host]), Duration::from_millis(250));
    let reading = wait_for(&monitor, &host, |r| r.unresolved);
    assert!(reading.history.is_empty());
}
