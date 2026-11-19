//! Round trips to a host for the ping widget and the `net.ping*` readings.
//!
//! Probes are ICMP echo requests on the kernel's unprivileged ping sockets
//! (`SOCK_DGRAM` with `IPPROTO_ICMP` or `IPPROTO_ICMPV6`), which Linux opens
//! for the groups in `net.ipv4.ping_group_range`. On such a socket the
//! kernel fills in the identifier (the socket's port) and the checksum of
//! every request, refuses anything but echo requests, and hands this socket
//! only the replies meant for it, without the IP header. Replies are told
//! apart by their sequence number. Where the system does not let this user
//! open a ping socket, a probe times a TCP connection to port 443 instead.
//!
//! Every host gets a thread of its own that probes once per interval, so a
//! slow DNS lookup or a host that does not answer never holds up anything
//! else. The threads run only while a host is wanted.

mod probe;
#[cfg(test)]
mod tests;

use probe::{icmp_denied, icmp_round_trip, open_icmp, tcp_round_trip, Outcome, Prober};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Probes kept for the statistics and the graph.
pub const WINDOW: usize = 60;
/// A probe without a reply within this long counts as lost, unless a
/// widget's red threshold asks for longer (see [`probe_timeout`]).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
/// How much longer than its red threshold a widget waits for an answer.
const TIMEOUT_PAST_RED: Duration = Duration::from_millis(500);
/// This many lost probes in a row mean the host does not answer at all.
pub const DOWN_AFTER: u32 = 3;
/// DNS answers change; a working host name is looked up again this often...
const RESOLVE_EVERY: Duration = Duration::from_secs(300);
/// ...sooner while its probes go unanswered...
const RESOLVE_WHEN_LOST: Duration = Duration::from_secs(30);
const LOST_BEFORE_RESOLVE: u32 = 5;
/// ...and this often while the name does not resolve at all.
const RESOLVE_RETRY: Duration = Duration::from_secs(10);
/// After this many unanswered probes in a row the next address of the
/// host gets a turn, with a fresh socket.
const SWITCH_AFTER_LOST: u32 = 5;

/// How the round trips are measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PingMethod {
    /// ICMP echo.
    #[default]
    Icmp,
    /// Connecting to TCP port 443; a refused connection counts as an answer.
    Tcp,
}

/// One host as last measured.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PingReading {
    pub host: String,
    pub method: PingMethod,
    /// The name has not resolved to an address (yet).
    pub unresolved: bool,
    /// The host is this computer's default gateway (IPv4).
    pub gateway: bool,
    /// Round trip of each of the last probes in milliseconds, oldest first;
    /// `None` for a lost probe.
    pub history: Vec<Option<f32>>,
}

impl PingReading {
    pub fn stats(&self) -> PingStats {
        stats(&self.history)
    }
}

/// Statistics over the probes of a [`PingReading`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PingStats {
    /// The latest answered probe.
    pub last_ms: Option<f32>,
    pub avg_ms: Option<f32>,
    pub min_ms: Option<f32>,
    pub max_ms: Option<f32>,
    /// Mean absolute difference between consecutive answered probes.
    pub jitter_ms: Option<f32>,
    /// Share of lost probes; `None` before the first probe.
    pub loss_percent: Option<f32>,
    /// Lost probes since the last answer.
    pub lost_in_row: u32,
}

impl PingStats {
    /// The last [`DOWN_AFTER`] probes all went unanswered.
    pub fn down(&self) -> bool {
        self.lost_in_row >= DOWN_AFTER
    }
}

/// The timeout for a host whose widgets turn red at `red_ms`: long enough
/// that a red round trip shows as one instead of as a lost probe.
pub fn probe_timeout(red_ms: Option<u32>) -> Duration {
    let red = Duration::from_millis(u64::from(red_ms.unwrap_or(0)));
    PROBE_TIMEOUT.max(red + TIMEOUT_PAST_RED)
}

/// `addr` as a probe can reach it: IPv4-mapped IPv6 addresses as IPv4
/// (ICMPv6 refuses them), and `None` for an IPv6 link-local address, which
/// needs an interface to go out on.
pub fn probe_address(addr: IpAddr) -> Option<IpAddr> {
    let addr = addr.to_canonical();
    match addr {
        IpAddr::V6(v6) if v6.is_unicast_link_local() => None,
        _ => Some(addr),
    }
}

pub fn stats(history: &[Option<f32>]) -> PingStats {
    let answered: Vec<f32> = history.iter().flatten().copied().collect();
    let lost = history.len() - answered.len();
    let lost_in_row = history
        .iter()
        .rev()
        .take_while(|probe| probe.is_none())
        .count() as u32;
    let jitter_ms = (answered.len() >= 2).then(|| {
        let steps: f32 = answered.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
        steps / (answered.len() - 1) as f32
    });
    let avg_ms =
        (!answered.is_empty()).then(|| answered.iter().sum::<f32>() / answered.len() as f32);
    PingStats {
        last_ms: answered.last().copied(),
        avg_ms,
        min_ms: answered.iter().copied().reduce(f32::min),
        max_ms: answered.iter().copied().reduce(f32::max),
        jitter_ms,
        loss_percent: (!history.is_empty()).then(|| lost as f32 * 100.0 / history.len() as f32),
        lost_in_row,
    }
}

/// The address of the default IPv4 route with the lowest metric, from
/// `/proc/net/route`.
pub fn default_gateway() -> Option<Ipv4Addr> {
    parse_default_gateway(&std::fs::read_to_string("/proc/net/route").ok()?)
}

pub fn parse_default_gateway(table: &str) -> Option<Ipv4Addr> {
    const RTF_UP: u32 = 0x1;
    const RTF_GATEWAY: u32 = 0x2;
    let hex = |field: &str| u32::from_str_radix(field, 16).ok();
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (destination, gateway, flags) = (
                hex(fields.get(1)?)?,
                hex(fields.get(2)?)?,
                hex(fields.get(3)?)?,
            );
            let (metric, mask) = (fields.get(6)?.parse::<u32>().ok()?, hex(fields.get(7)?)?);
            let default = destination == 0 && mask == 0 && gateway != 0;
            (default && flags & (RTF_UP | RTF_GATEWAY) == RTF_UP | RTF_GATEWAY)
                // The kernel prints the address bytes as a number in host
                // order.
                .then(|| (metric, Ipv4Addr::from(gateway.to_ne_bytes())))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, gateway)| gateway)
}

/// Where one host's probes go, and when its name is looked up again.
struct Target {
    host: String,
    literal: Option<IpAddr>,
    /// What the name resolved to, in the resolver's order.
    addrs: Vec<IpAddr>,
    /// The address probed now.
    current: usize,
    resolved_at: Option<Instant>,
    failed_at: Option<Instant>,
    prober: Option<Prober>,
    method: PingMethod,
    gateway: bool,
    lost_in_row: u32,
}

impl Target {
    fn new(host: &str) -> Self {
        Self {
            host: host.to_string(),
            literal: host.parse().ok(),
            addrs: Vec::new(),
            current: 0,
            resolved_at: None,
            failed_at: None,
            prober: None,
            method: PingMethod::Icmp,
            gateway: false,
            lost_in_row: 0,
        }
    }

    fn addr(&self) -> Option<IpAddr> {
        self.addrs.get(self.current).copied()
    }

    /// Looks the host up when that is due. Blocks for as long as DNS takes.
    fn refresh(&mut self, now: Instant) {
        let since = |at: Option<Instant>| at.map(|at| now.saturating_duration_since(at));
        let due = match (self.addr(), since(self.resolved_at)) {
            (None, _) => since(self.failed_at).is_none_or(|ago| ago >= RESOLVE_RETRY),
            (Some(_), Some(ago)) => {
                ago >= RESOLVE_EVERY
                    || (self.lost_in_row >= LOST_BEFORE_RESOLVE && ago >= RESOLVE_WHEN_LOST)
            }
            (Some(_), None) => true,
        };
        if !due {
            return;
        }
        let found: Vec<IpAddr> = match self.literal {
            Some(addr) => vec![addr],
            None => (self.host.as_str(), 0)
                .to_socket_addrs()
                .map(|addrs| addrs.map(|addr| addr.ip()).collect())
                .unwrap_or_default(),
        };
        let mut addrs: Vec<IpAddr> = Vec::new();
        for addr in found.into_iter().filter_map(probe_address) {
            if !addrs.contains(&addr) {
                addrs.push(addr);
            }
        }
        if addrs.is_empty() {
            // A failed lookup keeps the addresses that worked before.
            self.failed_at = Some(now);
            if !self.addrs.is_empty() {
                self.resolved_at = Some(now);
            }
            return;
        }
        let before = self.addr();
        self.current = before
            .and_then(|addr| addrs.iter().position(|a| *a == addr))
            .unwrap_or(0);
        self.addrs = addrs;
        self.resolved_at = Some(now);
        self.failed_at = None;
        if self.addr() != before {
            self.prober = None;
        }
        self.note_gateway();
    }

    fn note_gateway(&mut self) {
        self.gateway = self
            .addr()
            .is_some_and(|addr| default_gateway().is_some_and(|gw| IpAddr::V4(gw) == addr));
    }

    /// One probe; `None` when it went unanswered.
    fn probe(&mut self, seq: u16, timeout: Duration) -> Option<Duration> {
        let addr = self.addr()?;
        if self.prober.is_none() {
            self.prober = match open_icmp(addr.is_ipv6()) {
                Ok(socket) => Some(Prober::Icmp {
                    socket,
                    v6: addr.is_ipv6(),
                }),
                Err(error) if icmp_denied(&error) => Some(Prober::Tcp),
                // Tried again with the next probe.
                Err(_) => None,
            };
        }
        let outcome = match self.prober.as_ref() {
            Some(Prober::Icmp { socket, v6 }) if *v6 == addr.is_ipv6() => {
                self.method = PingMethod::Icmp;
                icmp_round_trip(socket, addr, seq, timeout)
            }
            Some(Prober::Tcp) => {
                self.method = PingMethod::Tcp;
                tcp_round_trip(addr, timeout)
            }
            _ => Outcome::Failed,
        };
        let Outcome::Answered(round_trip) = outcome else {
            self.lost_in_row += 1;
            if outcome == Outcome::Failed {
                // Opened afresh for the next probe.
                self.prober = None;
            }
            // A host that stopped answering may be reachable another way
            // (IPv4 where IPv6 is broken), and a fresh socket also picks up
            // a ping_group_range that changed.
            if self.lost_in_row.is_multiple_of(SWITCH_AFTER_LOST) {
                self.current = (self.current + 1) % self.addrs.len();
                self.prober = None;
                self.note_gateway();
            }
            return None;
        };
        self.lost_in_row = 0;
        Some(round_trip)
    }
}

#[derive(Default)]
struct State {
    interval: Duration,
    hosts: HashMap<String, Slot>,
    generation: u64,
    stopped: bool,
}

struct Slot {
    /// Tells the slot's thread from an earlier one for the same host.
    generation: u64,
    timeout: Duration,
    reading: PingReading,
    probes: VecDeque<Option<f32>>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Pings the wanted hosts in the background.
pub struct PingMonitor {
    shared: Arc<Shared>,
}

impl Default for PingMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl PingMonitor {
    pub fn new() -> Self {
        Self {
            shared: Arc::default(),
        }
    }

    /// Probes each host every `interval`, waiting up to its timeout for
    /// an answer: starts a thread for each new host and lets the threads of
    /// hosts no longer listed end, with their readings.
    pub fn set_hosts(&self, hosts: &[(String, Duration)], interval: Duration) {
        let mut state = self.shared.lock();
        let changed = state.interval != interval
            || state.hosts.len() != hosts.len()
            || hosts.iter().any(|(host, timeout)| {
                state
                    .hosts
                    .get(host)
                    .is_none_or(|slot| slot.timeout != *timeout)
            });
        if !changed {
            return;
        }
        state.interval = interval;
        state
            .hosts
            .retain(|host, _| hosts.iter().any(|(wanted, _)| wanted == host));
        for (host, timeout) in hosts {
            if let Some(slot) = state.hosts.get_mut(host) {
                slot.timeout = *timeout;
                continue;
            }
            state.generation += 1;
            let generation = state.generation;
            let reading = PingReading {
                host: host.clone(),
                ..PingReading::default()
            };
            let shared = self.shared.clone();
            let owned = host.clone();
            let spawned = thread::Builder::new()
                .name("ping".into())
                .spawn(move || run(&shared, &owned, generation));
            if spawned.is_ok() {
                state.hosts.insert(
                    host.clone(),
                    Slot {
                        generation,
                        timeout: *timeout,
                        reading,
                        probes: VecDeque::with_capacity(WINDOW),
                    },
                );
            }
        }
        // Waiting threads pick up the interval, or notice they are done.
        self.shared.wake.notify_all();
    }

    pub fn reading(&self, host: &str) -> Option<PingReading> {
        self.shared
            .lock()
            .hosts
            .get(host)
            .map(|slot| slot.reading.clone())
    }

    pub fn readings(&self) -> Vec<PingReading> {
        let mut readings: Vec<PingReading> = self
            .shared
            .lock()
            .hosts
            .values()
            .map(|slot| slot.reading.clone())
            .collect();
        readings.sort_by(|a, b| a.host.cmp(&b.host));
        readings
    }
}

impl Drop for PingMonitor {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.stopped = true;
        state.hosts.clear();
        self.shared.wake.notify_all();
    }
}

/// One host's probe loop; ends once the host is no longer wanted.
fn run(shared: &Shared, host: &str, generation: u64) {
    let current = |state: &State| {
        !state.stopped
            && state
                .hosts
                .get(host)
                .is_some_and(|slot| slot.generation == generation)
    };
    let mut target = Target::new(host);
    let mut seq: u16 = 0;
    loop {
        let started = Instant::now();
        let Some(timeout) = shared.lock().hosts.get(host).map(|slot| slot.timeout) else {
            return;
        };
        target.refresh(started);
        let round_trip = target.probe(seq, timeout);
        seq = seq.wrapping_add(1);
        let mut state = shared.lock();
        if !current(&state) {
            return;
        }
        if let Some(slot) = state.hosts.get_mut(host) {
            let reading = &mut slot.reading;
            reading.unresolved = target.addr().is_none();
            reading.method = target.method;
            reading.gateway = target.gateway;
            // Without an address nothing was sent, so nothing was lost.
            if target.addr().is_some() {
                if slot.probes.len() == WINDOW {
                    slot.probes.pop_front();
                }
                slot.probes
                    .push_back(round_trip.map(|rtt| rtt.as_secs_f32() * 1000.0));
                reading.history = slot.probes.iter().copied().collect();
            }
        }
        // The next probe is due one interval after this one started, and
        // never before it finished: a lost probe holds the next one back
        // until its timeout ran out.
        loop {
            if !current(&state) {
                return;
            }
            let wait = (started + state.interval).saturating_duration_since(Instant::now());
            if wait.is_zero() {
                break;
            }
            state = shared
                .wake
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}
