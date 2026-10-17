//! The world: a discrete-event simulation on a virtual clock (ms). Nodes
//! run the real kernel; the network delivers real wire frames with
//! latency, loss and bandwidth; miners find blocks at their simulated
//! hashrate share; agents send transactions; a script changes the world
//! (partitions, crashes, hashrate steps); invariants are sampled
//! continuously. Every tie is broken by insertion order and every map is
//! ordered, so a seed reproduces a run.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use soft3::chain::net::chain_view::{BlockVerdict, ChainView};
use soft3::chain::net::codec::{Header, U256 as WU256};
use soft3::chain::net::frames::Checkpoint;
use soft3::chain::params::{FINALITY_DEPTH, MAX_REORG};

use crate::check::{Checks, Status};
use crate::metrics::Metrics;
use crate::net::{LinkSpec, Net, Wire};
use crate::node::{Config, Role, SimNode, TICK_MS};
use crate::rng::Rng;

pub type Id = [u8; 32];

pub const SEC: u64 = 1_000;
pub const MIN: u64 = 60 * SEC;
pub const HOUR: u64 = 60 * MIN;

/// What a miner does with what it finds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Strategy {
    Honest,
    /// Eyal–Sirer SM1 withholding
    Selfish { race: bool },
    /// mine privately from the node's tip; release once `depth` deep and heavier
    Private { depth: u64, fork: Option<u64>, released: bool },
}

pub struct Miner {
    pub name: String,
    pub node: usize,
    pub to: Id,
    /// simulated hashrate, milli-hashes per second
    pub rate_mhs: u64,
    pub gen: u64,
    pub strategy: Strategy,
    pub found: u64,
    pub hashes: u64,
    pub ground_us: u64,
}

pub trait Agent {
    fn name(&self) -> String;
    /// Act; return the delay to the next action, or `None` to stop.
    fn act(&mut self, w: &mut World) -> Option<u64>;
    /// Checks the agent owns, evaluated at the end of the run.
    fn finish(&mut self, _w: &mut World) {}
}

pub type Action = Box<dyn FnOnce(&mut World)>;

enum Ev {
    Deliver { to: usize, from: usize, wire: Wire },
    Tick(usize),
    Mine { miner: usize, gen: u64 },
    Agent(usize),
    Script(usize),
    Sample,
    Wake(usize),
    Sign,
}

pub struct World {
    pub now: u64,
    pub scenario: String,
    seq: u64,
    queue: BinaryHeap<Reverse<(u64, u64)>>,
    events: BTreeMap<u64, Ev>,
    pub nodes: Vec<SimNode>,
    pub net: Net,
    pub miners: Vec<Miner>,
    agents: Vec<Option<Box<dyn Agent>>>,
    script: Vec<Option<Action>>,
    pub rng: Rng,
    pub checks: Checks,
    pub metrics: Metrics,
    pub cfg: Arc<Config>,
    /// block id → (virtual ms found, miner)
    pub found: BTreeMap<Id, (u64, usize)>,
    pub sample_every: u64,
    /// nodes whose divergence from the others is expected (eclipse victims, partitions)
    pub expect_split: Vec<usize>,
    /// largest virtual gap without a new tip on an honest online node that is tolerated
    pub stall_ms: u64,
    pub signer: Option<(usize, mudra::SigningKey, Id)>,
    signed: u64,
    pub mining: bool,
    pub wall: Instant,
    pub deadline: u64,
    pub hash_budget: u64,
    pub log: bool,
    pub notes: Vec<String>,
    /// bans of honest peers are part of the scenario (not a failure)
    pub allow_honest_bans: bool,
    /// the hostile nodes of the run (their bans are the point)
    pub hostile_ix: Vec<usize>,
    /// grinds a valid nonce for real difficulty (the launch suite); `None` = the scalar loop
    pub grinder: Option<Box<dyn Fn(&mut Header) -> u64>>,
}

impl World {
    pub fn new(scenario: &str, seed: u64, cfg: Arc<Config>, metrics: Metrics) -> World {
        World {
            now: 0,
            scenario: scenario.to_string(),
            seq: 0,
            queue: BinaryHeap::new(),
            events: BTreeMap::new(),
            nodes: Vec::new(),
            net: Net::default(),
            miners: Vec::new(),
            agents: Vec::new(),
            script: Vec::new(),
            rng: Rng::new(seed),
            checks: Checks::new(scenario),
            metrics,
            cfg,
            found: BTreeMap::new(),
            sample_every: MIN,
            expect_split: Vec::new(),
            stall_ms: 20 * MIN,
            signer: None,
            signed: 0,
            mining: true,
            wall: Instant::now(),
            deadline: 0,
            hash_budget: 0,
            log: false,
            notes: Vec::new(),
            allow_honest_bans: false,
            hostile_ix: Vec::new(),
            grinder: None,
        }
    }

    fn push(&mut self, at: u64, ev: Ev) {
        let s = self.seq;
        self.seq += 1;
        self.events.insert(s, ev);
        self.queue.push(Reverse((at, s)));
    }

    pub fn at(&mut self, t: u64, f: impl FnOnce(&mut World) + 'static) {
        let i = self.script.len();
        self.script.push(Some(Box::new(f)));
        self.push(t, Ev::Script(i));
    }

    pub fn add_agent(&mut self, start: u64, a: Box<dyn Agent>) {
        let i = self.agents.len();
        self.agents.push(Some(a));
        self.push(start, Ev::Agent(i));
    }

    // ── topology ───────────────────────────────────────────────────

    pub fn add_node(&mut self, name: &str, home: PathBuf) -> usize {
        let ix = self.nodes.len();
        let mut n = SimNode::new(ix, name.to_string(), home, self.cfg.clone());
        n.now_ms = self.now;
        self.nodes.push(n);
        ix
    }

    /// Bring a node online (first start or restart after a crash) and link it.
    pub fn start_node(&mut self, ix: usize) {
        self.nodes[ix].now_ms = self.now;
        match self.nodes[ix].start(self.now) {
            Ok(ms) => {
                if ms > 2_000 {
                    self.notes.push(format!("{} opened in {ms} ms", self.nodes[ix].name));
                }
            }
            Err(e) => {
                self.checks.fail("node-start", &format!("{}: {e}", self.nodes[ix].name));
                return;
            }
        }
        for p in self.net.neighbors(ix) {
            if self.nodes[p].online {
                self.nodes[ix].connected(p);
                self.nodes[p].connected(ix);
            }
        }
        let jitter = self.rng.below(TICK_MS);
        self.push(self.now + jitter, Ev::Tick(ix));
        self.nodes[ix].ask_checkpoints();
        self.nodes[ix].sync_pass(None);
        self.flush(ix);
        self.reschedule_node(ix);
    }

    pub fn crash_node(&mut self, ix: usize) {
        self.nodes[ix].crash();
        for p in self.net.all_links_of(ix) {
            self.nodes[p].disconnected(ix);
        }
    }

    pub fn link(&mut self, a: usize, b: usize, spec: LinkSpec) {
        self.net.connect(a, b, spec);
        if self.nodes[a].online && self.nodes[b].online {
            self.nodes[a].connected(b);
            self.nodes[b].connected(a);
        }
    }

    pub fn unlink(&mut self, a: usize, b: usize) {
        self.net.disconnect(a, b);
        self.nodes[a].disconnected(b);
        self.nodes[b].disconnected(a);
    }

    pub fn partition(&mut self, groups: &[Vec<usize>]) {
        self.net.partition(groups);
        for a in 0..self.nodes.len() {
            for b in self.net.all_links_of(a) {
                if !self.net.same_side(a, b) {
                    self.nodes[a].disconnected(b);
                }
            }
        }
    }

    pub fn heal(&mut self) {
        self.net.heal();
        for a in 0..self.nodes.len() {
            for b in self.net.neighbors(a) {
                if self.nodes[a].online && self.nodes[b].online {
                    self.nodes[a].connected(b);
                }
            }
        }
        for a in 0..self.nodes.len() {
            if self.nodes[a].online {
                self.nodes[a].sync_pass(None);
                self.flush(a);
            }
        }
    }

    pub fn add_miner(&mut self, name: &str, node: usize, rate_mhs: u64, strategy: Strategy) -> usize {
        let to = *hemera::hash(format!("miner/{name}").as_bytes()).as_bytes();
        self.add_miner_to(name, node, rate_mhs, strategy, to)
    }

    pub fn add_miner_to(&mut self, name: &str, node: usize, rate_mhs: u64, strategy: Strategy, to: Id) -> usize {
        let ix = self.miners.len();
        self.miners.push(Miner { name: name.into(), node, to, rate_mhs, gen: 0, strategy, found: 0, hashes: 0, ground_us: 0 });
        self.reschedule(ix);
        ix
    }

    pub fn set_rate(&mut self, miner: usize, rate_mhs: u64) {
        self.miners[miner].rate_mhs = rate_mhs;
        self.reschedule(miner);
    }

    // ── mining ─────────────────────────────────────────────────────

    fn reschedule(&mut self, m: usize) {
        self.miners[m].gen += 1;
        let node = self.miners[m].node;
        if !self.mining || !self.nodes[node].online || self.miners[m].rate_mhs == 0 {
            return;
        }
        let bits = self.nodes[node].chain().next_bits();
        let target = soft3::chain::net::codec::decode_bits(bits).unwrap_or(WU256::pow2(236));
        let work = soft3::chain::net::codec::work(target);
        let work = if work.bits() > 63 { u64::MAX / 4 } else { work.0[0] };
        // expected time = work / rate; rate in mH/s → ms = work · 10^6 / rate
        let mean_ms = ((work as u128 * 1_000_000) / self.miners[m].rate_mhs as u128).min(u64::MAX as u128 / 2) as u64;
        let mut after = self.rng.exp(mean_ms.max(1));
        if let Some(ready) = self.nodes[node].template_ready_at() {
            after = after.max(ready - self.now);
        }
        let gen = self.miners[m].gen;
        self.push(self.now + after, Ev::Mine { miner: m, gen });
    }

    fn reschedule_node(&mut self, node: usize) {
        let ms: Vec<usize> = (0..self.miners.len()).filter(|m| self.miners[*m].node == node).collect();
        for m in ms {
            self.reschedule(m);
        }
    }

    /// Grind a genuinely valid nonce for `header` (the simulated hashrate
    /// decided *when*; the kernel checks the work like any other block).
    pub fn grind(header: &mut Header) -> u64 {
        for n in 0u128.. {
            header.nonce = n.to_le_bytes();
            if header.valid_pow() {
                return n as u64 + 1;
            }
        }
        unreachable!()
    }

    fn on_mine(&mut self, m: usize) {
        let node = self.miners[m].node;
        if !self.nodes[node].online {
            return;
        }
        if let Some(ready) = self.nodes[node].template_ready_at() {
            let gen = self.miners[m].gen;
            self.push(ready, Ev::Mine { miner: m, gen });
            return;
        }
        let to = self.miners[m].to;
        let n = &mut self.nodes[node];
        let next_height = n.tip().height + 1;
        let txs = n.mempool.template_txs();
        let now = n.now();
        let t0 = Instant::now();
        let chain = n.chain.as_ref().expect("online");
        let tpl = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| chain.template(&to, &txs, now))) {
            Ok(t) => t,
            Err(_) => {
                n.stats.panics.push(format!("template at {next_height}"));
                n.crash();
                self.crashed_by_panic(node);
                return;
            }
        };
        n.stats.template_us_max = n.stats.template_us_max.max(t0.elapsed().as_micros() as u64);
        let (mut header, body) = match tpl {
            Ok(t) => t,
            Err(e) => {
                self.checks.fail("template", &format!("{}: {e}", n.name));
                return;
            }
        };
        let t1 = Instant::now();
        let hashes = match &self.grinder {
            Some(g) => g(&mut header),
            None => Self::grind(&mut header),
        };
        self.miners[m].ground_us += t1.elapsed().as_micros() as u64;
        self.miners[m].hashes += hashes;
        self.hash_budget += hashes;
        let id = header.id();
        let verdict = self.nodes[node].accept_block(node, &header, &body);
        match verdict {
            Ok(BlockVerdict::NewTip) => {}
            other => {
                self.checks.fail("own-block", &format!("{} rejected its own block at {}: {other:?}", self.nodes[node].name, header.height));
                return;
            }
        }
        self.miners[m].found += 1;
        self.found.insert(id, (self.now, m));
        let publish = match &mut self.miners[m].strategy {
            Strategy::Honest => true,
            Strategy::Selfish { .. } | Strategy::Private { .. } => false,
        };
        if publish {
            self.nodes[node].announce(&header);
        } else {
            self.nodes[node].private.insert(id);
            self.on_private_found(m);
        }
        self.flush(node);
        self.after_tip(node);
    }

    /// The withholding strategies' bookkeeping after the attacker finds a block.
    fn on_private_found(&mut self, m: usize) {
        let node = self.miners[m].node;
        match self.miners[m].strategy.clone() {
            Strategy::Selfish { race } => {
                if race {
                    self.release(node, u64::MAX);
                    self.miners[m].strategy = Strategy::Selfish { race: false };
                }
            }
            Strategy::Private { depth, fork, released } => {
                let tip = self.nodes[node].tip();
                let fork_h = fork.unwrap_or(tip.height - 1);
                if fork.is_none() {
                    self.miners[m].strategy = Strategy::Private { depth, fork: Some(fork_h), released };
                }
                // heavier than every honest node's tip and deep enough: release
                let honest_best = self.honest_best_work(node);
                if !released && tip.height - fork_h >= depth && tip.chainwork > honest_best {
                    self.nodes[node].deaf = false;
                    self.release(node, u64::MAX);
                    self.miners[m].strategy = Strategy::Private { depth, fork: Some(fork_h), released: true };
                    self.notes.push(format!("private chain released: fork at {fork_h}, tip {}, depth {}", tip.height, tip.height - fork_h));
                }
            }
            Strategy::Honest => {}
        }
    }

    fn honest_best_work(&self, except: usize) -> WU256 {
        self.nodes
            .iter()
            .filter(|n| n.online && n.ix != except && n.role == Role::Honest)
            .map(|n| n.tip().chainwork)
            .max()
            .unwrap_or(WU256::ZERO)
    }

    /// Publish a withholder's private blocks up to `height` (all for `u64::MAX`).
    pub fn release(&mut self, node: usize, height: u64) {
        let n = &mut self.nodes[node];
        let chain = n.chain.as_ref().expect("online");
        let mut ids: Vec<(u64, Id)> = n.private.iter().filter_map(|id| chain.header(id).map(|h| (h.height, *id))).filter(|(h, _)| *h <= height).collect();
        ids.sort();
        for (_, id) in &ids {
            n.private.remove(id);
        }
        if let Some((_, top)) = ids.last() {
            let h = chain.header(top).expect("held");
            n.announce(&h);
        }
        self.flush(node);
    }

    /// SM1 on an honest block reaching the selfish miner's node.
    fn selfish_react(&mut self, node: usize) {
        let ms: Vec<usize> = (0..self.miners.len()).filter(|m| self.miners[*m].node == node && matches!(self.miners[*m].strategy, Strategy::Selfish { .. })).collect();
        for m in ms {
            let n = &self.nodes[node];
            if n.private.is_empty() {
                continue;
            }
            let chain = n.chain();
            let v = n.private.iter().filter_map(|id| chain.header(id).map(|h| h.height)).max().unwrap_or(0);
            let p = n.foreign_best;
            if v < p {
                // lost: the node already follows the heavier public chain
                self.nodes[node].private.clear();
                continue;
            }
            match v - p {
                0 => {
                    self.release(node, u64::MAX);
                    self.miners[m].strategy = Strategy::Selfish { race: true };
                }
                1 => self.release(node, u64::MAX),
                _ => self.release(node, p),
            }
        }
    }

    // ── delivery ───────────────────────────────────────────────────

    /// Send everything a node queued.
    pub fn flush(&mut self, from: usize) {
        let out = std::mem::take(&mut self.nodes[from].outbox);
        for (to, wire) in out {
            if let Some(at) = self.net.send(&mut self.rng, self.now, from, to, wire.bytes()) {
                self.push(at, Ev::Deliver { to, from, wire });
            }
        }
    }

    fn after_tip(&mut self, node: usize) {
        if !std::mem::take(&mut self.nodes[node].tip_moved) {
            return;
        }
        self.reschedule_node(node);
        if self.nodes[node].role == Role::Withholder {
            self.selfish_react(node);
        }
        // deferred settlement work wakes on its own timer
        if let Some(t) = self.nodes[node].deferred.iter().map(|d| d.0).min() {
            self.push(t, Ev::Wake(node));
        }
    }

    // ── the loop ───────────────────────────────────────────────────

    pub fn run_until(&mut self, end: u64) {
        if self.events.is_empty() || !self.queue.iter().any(|_| true) {
            // first call
        }
        if self.now == 0 {
            self.push(self.sample_every, Ev::Sample);
            self.push(MIN, Ev::Sign);
        }
        while let Some(Reverse((t, s))) = self.queue.peek().copied() {
            if t > end {
                break;
            }
            self.queue.pop();
            let ev = self.events.remove(&s).expect("event");
            self.now = t;
            for n in self.nodes.iter_mut() {
                n.now_ms = t;
            }
            self.dispatch(ev);
            if self.deadline > 0 && self.wall.elapsed().as_secs() > self.deadline {
                self.checks.fail("wall-clock budget", &format!("stopped at virtual {} s after {} s of wall time", self.now / 1000, self.deadline));
                break;
            }
        }
        self.now = self.now.max(end);
        for n in self.nodes.iter_mut() {
            n.now_ms = self.now;
        }
    }

    fn dispatch(&mut self, ev: Ev) {
        match ev {
            Ev::Deliver { to, from, wire } => {
                if !self.nodes[to].online || (self.nodes[to].deaf && !matches!(wire, Wire::Req(..))) {
                    return;
                }
                if !self.net.linked(from, to) {
                    return;
                }
                self.nodes[to].on_wire(from, wire);
                self.flush(to);
                self.after_tip(to);
                if !self.nodes[to].online {
                    self.crashed_by_panic(to);
                }
            }
            Ev::Tick(n) => {
                if self.nodes[n].online {
                    self.nodes[n].tick();
                    self.flush(n);
                    self.after_tip(n);
                    if !self.nodes[n].online {
                        self.crashed_by_panic(n);
                        return;
                    }
                    self.push(self.now + TICK_MS, Ev::Tick(n));
                }
            }
            Ev::Wake(n) => {
                if self.nodes[n].online {
                    self.nodes[n].wake();
                    self.flush(n);
                    self.after_tip(n);
                }
            }
            Ev::Mine { miner, gen } => {
                if self.miners[miner].gen == gen && self.mining {
                    self.on_mine(miner);
                    if self.miners[miner].gen == gen {
                        self.reschedule(miner);
                    }
                }
            }
            Ev::Agent(i) => {
                if let Some(mut a) = self.agents[i].take() {
                    let next = a.act(self);
                    self.agents[i] = Some(a);
                    if let Some(d) = next {
                        self.push(self.now + d.max(1), Ev::Agent(i));
                    }
                }
            }
            Ev::Script(i) => {
                if let Some(f) = self.script[i].take() {
                    f(self);
                }
            }
            Ev::Sample => {
                self.sample();
                self.push(self.now + self.sample_every, Ev::Sample);
            }
            Ev::Sign => {
                self.sign_checkpoints();
                self.push(self.now + MIN, Ev::Sign);
            }
        }
    }

    fn crashed_by_panic(&mut self, n: usize) {
        let msg = self.nodes[n].stats.panics.last().cloned().unwrap_or_default();
        self.checks.fail("no-panic", &format!("{} crashed: {msg}", self.nodes[n].name));
        for p in self.net.all_links_of(n) {
            self.nodes[p].disconnected(n);
        }
    }

    /// The foundation signer (chain.md §5.3): a checkpoint every 120
    /// heights, at least `FINALITY_DEPTH` below its node's tip.
    fn sign_checkpoints(&mut self) {
        let Some((node, key, chain_id)) = self.signer.clone() else { return };
        if !self.nodes[node].online {
            return;
        }
        let tip = self.nodes[node].tip().height;
        while (self.signed + 1) * 120 + FINALITY_DEPTH <= tip {
            let h = (self.signed + 1) * 120;
            let Some(id) = self.nodes[node].chain().header_at(h).map(|x| x.id()) else { break };
            let cp = soft3::chain::select::Checkpoint::sign(&chain_id, h, id, &key).expect("sign");
            self.nodes[node].publish_checkpoint(Checkpoint { height: h, block_id: id, sig: cp.sig });
            self.flush(node);
            self.after_tip(node);
            self.signed += 1;
        }
    }

    // ── invariants sampled on the way ──────────────────────────────

    pub fn honest_online(&self) -> Vec<usize> {
        self.nodes.iter().filter(|n| n.online && n.role == Role::Honest && !self.expect_split.contains(&n.ix)).map(|n| n.ix).collect()
    }

    fn sample(&mut self) {
        let honest = self.honest_online();
        // liveness: an honest online node without a new tip for too long
        for &i in &honest {
            let gap = self.now - self.nodes[i].stats.last_tip_ms;
            if self.mining && gap > self.stall_ms {
                self.checks.fail("liveness: no stall", &format!("{} had no new tip for {} s at virtual {} s (height {})", self.nodes[i].name, gap / 1000, self.now / 1000, self.nodes[i].tip().height));
                self.nodes[i].stats.last_tip_ms = self.now; // report once per stall
            }
        }
        // safety: honest nodes agree below the finality depth
        for (k, &a) in honest.iter().enumerate() {
            for &b in &honest[k + 1..] {
                let (ta, tb) = (self.nodes[a].tip().height, self.nodes[b].tip().height);
                let h = ta.min(tb).saturating_sub(FINALITY_DEPTH);
                if h == 0 {
                    continue;
                }
                let ia = self.nodes[a].chain().header_at(h).map(|x| x.id());
                let ib = self.nodes[b].chain().header_at(h).map(|x| x.id());
                if ia != ib {
                    self.checks.fail("safety: agree below finality", &format!("{} and {} differ at height {h} (tips {ta}, {tb}) at virtual {} s", self.nodes[a].name, self.nodes[b].name, self.now / 1000));
                }
            }
        }
        for n in &self.nodes {
            if let Some((t, d)) = n.stats.reorgs.iter().find(|(_, d)| *d > MAX_REORG) {
                self.checks.fail("safety: reorg depth ≤ 119", &format!("{} reorganized {d} blocks at {} s", n.name, t / 1000));
            }
        }
        let tips: Vec<String> = self.nodes.iter().filter(|n| n.online).map(|n| n.short()).collect();
        let mempools: Vec<usize> = self.nodes.iter().filter(|n| n.online).map(|n| n.mempool.len()).collect();
        self.metrics.emit(serde_json::json!({"kind": "sample", "t": self.now / 1000, "tips": tips, "mempool": mempools, "rss_mb": rss_mb(), "hashes": self.hash_budget}));
        if self.log {
            eprintln!("  [{}] t={}s {}", self.scenario, self.now / 1000, tips.join(" "));
        }
    }

    /// Stop mining and agents, let the network settle, and check that every
    /// honest online node holds the same tip and the same state.
    pub fn quiesce(&mut self, ms: u64) {
        // equal-work tips stay split under first-seen until the next block:
        // one honest miner breaks the tie, then everything stops
        let breaker = (0..self.miners.len()).find(|m| self.miners[*m].strategy == Strategy::Honest && self.nodes[self.miners[*m].node].online && self.nodes[self.miners[*m].node].role == Role::Honest && !self.expect_split.contains(&self.miners[*m].node));
        for m in 0..self.miners.len() {
            if Some(m) != breaker {
                self.miners[m].rate_mhs = 0;
                self.miners[m].gen += 1;
            }
        }
        if let Some(b) = breaker {
            let start = self.miners[b].found;
            let mut guard = 0;
            while self.miners[b].found < start + 2 && guard < 200 {
                let t = self.now + 30 * SEC;
                self.run_until(t);
                guard += 1;
            }
        }
        self.mining = false;
        for m in 0..self.miners.len() {
            self.miners[m].gen += 1;
        }
        let end = self.now + ms;
        self.run_until(end);
    }

    pub fn converged(&mut self) -> bool {
        let honest = self.honest_online();
        let Some(&first) = honest.first() else { return true };
        let t0 = self.nodes[first].tip();
        let mut ok = true;
        for &i in &honest[1..] {
            let t = self.nodes[i].tip();
            if t.id != t0.id {
                ok = false;
                self.checks.fail("safety: converge after quiesce", &format!("{} at {} vs {} at {}", self.nodes[first].name, t0.height, self.nodes[i].name, t.height));
            }
        }
        // (every block a node applied had its state root recomputed and
        // compared by the kernel: equal tips are equal states)
        if ok {
            self.checks.pass("safety: converge after quiesce", &format!("{} honest nodes at height {} ({})", honest.len(), t0.height, soft3::chain::net::codec::hex(&t0.id[..4])));
        }
        ok
    }

    /// Main-chain blocks of `node` with their found time (virtual ms).
    pub fn main_chain(&self, node: usize) -> Vec<(u64, Header, Option<(u64, usize)>)> {
        let n = &self.nodes[node];
        let tip = n.tip().height;
        (1..=tip)
            .filter_map(|h| n.chain().header_at(h))
            .map(|hd| {
                let f = self.found.get(&hd.id()).copied();
                (hd.height, hd, f)
            })
            .collect()
    }

    /// Run digest for the determinism check: every node's main chain and
    /// final state root, the metrics stream, the check results.
    pub fn digest(&self) -> String {
        let mut buf = Vec::new();
        for n in &self.nodes {
            if !n.online {
                buf.extend_from_slice(b"offline");
                continue;
            }
            let t = n.tip();
            for id in n.main_ids(0, t.height) {
                buf.extend_from_slice(&id);
            }
            buf.extend_from_slice(&n.chain().state_root());
        }
        buf.extend_from_slice(&self.metrics.digest());
        soft3::chain::net::codec::hex(&hemera::hash(&buf).as_bytes()[..])
    }

    pub fn record(&mut self, invariant: &str, status: Status, evidence: String) {
        self.checks.add(invariant, status, evidence);
    }

    pub fn agents_finish(&mut self) {
        for i in 0..self.agents.len() {
            if let Some(mut a) = self.agents[i].take() {
                a.finish(self);
                self.agents[i] = Some(a);
            }
        }
    }
}

/// Resident set high-water mark of this process, MB.
pub fn rss_mb() -> u64 {
    #[cfg(unix)]
    {
        let mut ru: libc_rusage = unsafe { std::mem::zeroed() };
        // SAFETY: getrusage writes into the struct we own
        let r = unsafe { getrusage(0, &mut ru) };
        if r == 0 {
            let max = ru.ru_maxrss as u64;
            return if cfg!(target_os = "macos") { max / (1024 * 1024) } else { max / 1024 };
        }
    }
    0
}

#[cfg(unix)]
#[repr(C)]
#[allow(non_camel_case_types)]
struct libc_rusage {
    ru_utime: [i64; 2],
    ru_stime: [i64; 2],
    ru_maxrss: i64,
    rest: [i64; 13],
}

#[cfg(unix)]
extern "C" {
    fn getrusage(who: i32, usage: *mut libc_rusage) -> i32;
}
