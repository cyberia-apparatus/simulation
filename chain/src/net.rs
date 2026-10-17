//! The simulated network: undirected links between nodes with latency,
//! jitter, loss and bandwidth (a per-direction serialization queue),
//! partitions into groups, and links taken down and up (bans, eclipses,
//! crashes). Pure bookkeeping; the world schedules what it returns.

use std::collections::{BTreeMap, BTreeSet};

use crate::rng::Rng;

/// What travels between two nodes: gossip, or a request/response pair
/// correlated by id (the bi-stream of chain-wire §1). Every payload is a
/// list of real chain-wire frames, encoded.
#[derive(Clone, Debug)]
pub enum Wire {
    Gossip(Vec<u8>),
    Req(u64, Vec<Vec<u8>>),
    Resp(u64, Vec<Vec<u8>>),
}

impl Wire {
    pub fn bytes(&self) -> usize {
        // a stream or datagram costs ~40 bytes of framing on QUIC
        40 + match self {
            Wire::Gossip(b) => b.len(),
            Wire::Req(_, f) | Wire::Resp(_, f) => f.iter().map(|x| x.len()).sum(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LinkSpec {
    pub latency_ms: u64,
    pub jitter_ms: u64,
    pub loss_ppm: u64,
    /// bytes per second each way
    pub bandwidth: u64,
}

impl LinkSpec {
    pub const WAN: LinkSpec = LinkSpec { latency_ms: 60, jitter_ms: 40, loss_ppm: 2_000, bandwidth: 2_500_000 };
    pub const SLOW: LinkSpec = LinkSpec { latency_ms: 250, jitter_ms: 150, loss_ppm: 20_000, bandwidth: 200_000 };
}

#[derive(Clone, Debug)]
struct Link {
    spec: LinkSpec,
    up: bool,
}

#[derive(Default)]
pub struct Net {
    links: BTreeMap<(usize, usize), Link>,
    busy_until: BTreeMap<(usize, usize), u64>,
    group: BTreeMap<usize, u32>,
    pub sent: u64,
    pub dropped: u64,
    pub bytes: u64,
}

fn key(a: usize, b: usize) -> (usize, usize) {
    if a < b { (a, b) } else { (b, a) }
}

impl Net {
    pub fn connect(&mut self, a: usize, b: usize, spec: LinkSpec) {
        if a != b {
            self.links.insert(key(a, b), Link { spec, up: true });
        }
    }

    pub fn disconnect(&mut self, a: usize, b: usize) {
        self.links.remove(&key(a, b));
    }

    pub fn set_up(&mut self, a: usize, b: usize, up: bool) {
        if let Some(l) = self.links.get_mut(&key(a, b)) {
            l.up = up;
        }
    }

    pub fn linked(&self, a: usize, b: usize) -> bool {
        self.links.get(&key(a, b)).is_some_and(|l| l.up) && self.same_side(a, b)
    }

    pub fn neighbors(&self, a: usize) -> Vec<usize> {
        self.links
            .iter()
            .filter(|(_, l)| l.up)
            .filter_map(|((x, y), _)| if *x == a { Some(*y) } else if *y == a { Some(*x) } else { None })
            .filter(|b| self.same_side(a, *b))
            .collect()
    }

    /// Every link of `a`, up or down, partitioned or not.
    pub fn all_links_of(&self, a: usize) -> Vec<usize> {
        self.links.keys().filter_map(|(x, y)| if *x == a { Some(*y) } else if *y == a { Some(*x) } else { None }).collect()
    }

    /// Split the nodes into groups; nodes absent from every group are group 0.
    pub fn partition(&mut self, groups: &[Vec<usize>]) {
        self.group.clear();
        for (g, members) in groups.iter().enumerate() {
            for n in members {
                self.group.insert(*n, g as u32 + 1);
            }
        }
    }

    pub fn heal(&mut self) {
        self.group.clear();
    }

    pub fn same_side(&self, a: usize, b: usize) -> bool {
        self.group.get(&a).copied().unwrap_or(0) == self.group.get(&b).copied().unwrap_or(0)
    }

    /// Arrival time of a message sent now, or `None` if it is lost.
    pub fn send(&mut self, rng: &mut Rng, now: u64, from: usize, to: usize, size: usize) -> Option<u64> {
        self.sent += 1;
        let Some(link) = self.links.get(&key(from, to)).cloned() else {
            self.dropped += 1;
            return None;
        };
        if !link.up || !self.same_side(from, to) || rng.chance_ppm(link.spec.loss_ppm) {
            self.dropped += 1;
            return None;
        }
        self.bytes += size as u64;
        let tx_ms = (size as u64 * 1000).div_ceil(link.spec.bandwidth.max(1));
        let free = self.busy_until.entry((from, to)).or_insert(0);
        let start = (*free).max(now);
        *free = start + tx_ms;
        let jitter = rng.below(link.spec.jitter_ms + 1);
        Some(start + tx_ms + link.spec.latency_ms + jitter)
    }

    /// Random `degree`-regular-ish topology over `nodes`, connected (a ring
    /// plus random chords), every link `spec`.
    pub fn mesh(&mut self, rng: &mut Rng, nodes: &[usize], degree: usize, spec: LinkSpec) {
        let n = nodes.len();
        if n < 2 {
            return;
        }
        for i in 0..n {
            self.connect(nodes[i], nodes[(i + 1) % n], spec);
        }
        let mut have: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for (a, b) in self.links.keys() {
            have.entry(*a).or_default().insert(*b);
            have.entry(*b).or_default().insert(*a);
        }
        for &a in nodes {
            let mut tries = 0;
            while have.get(&a).map_or(0, |s| s.len()) < degree && tries < 8 * degree {
                tries += 1;
                let b = nodes[rng.below(n as u64) as usize];
                if b == a || have.get(&a).is_some_and(|s| s.contains(&b)) {
                    continue;
                }
                self.connect(a, b, spec);
                have.entry(a).or_default().insert(b);
                have.entry(b).or_default().insert(a);
            }
        }
    }
}
