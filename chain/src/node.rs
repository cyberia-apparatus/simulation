//! One simulated node: the real kernel behind the wire's port
//! (`soft3::chain::net::kernel::KernelChain`), the real mempool
//! (`net::mempool::Mempool`), real chain-wire frames, and the protocol
//! state machines of `net/gossip.rs`, `net/sync.rs` and `net/serve.rs`
//! re-expressed as event handlers over the simulated network. What is not
//! the real code: QUIC streams (a request/response pair with an id), the
//! iroh-gossip mesh (broadcast to the node's links), the per-peer token
//! buckets (they read the wall clock), and discovery (the scenario wires
//! the topology).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use soft3::chain::engine::OpenOptions;
use soft3::chain::net::chain_view::{BlockError, BlockVerdict, ChainEvent, ChainView, HeaderVerdict};
use soft3::chain::net::codec::{txid, Header, U256};
use soft3::chain::net::frames::{self, Checkpoint, Frame, FrameError, MAX_BLOCKS_BY_ID, MAX_GOSSIP_BYTES, MAX_HEADERS_PER_RESPONSE, MAX_RANGE};
use soft3::chain::net::kernel::KernelChain;
use soft3::chain::net::mempool::{AdmitError, Mempool};
use soft3::chain::params::{EpochClock, FINALITY_DEPTH};
use tokio::sync::broadcast;

use crate::net::Wire;

pub type Id = [u8; 32];

pub const BAN_SCORE: u32 = soft3::chain::net::peers::BAN_SCORE;
pub const BAN_MS: u64 = soft3::chain::net::peers::BAN_SECS * 1000;
pub const TICK_MS: u64 = soft3::chain::net::sync::TICK.as_millis() as u64;
pub const REQUEST_TIMEOUT_MS: u64 = 20_000;

#[derive(Clone, Debug, Default)]
pub struct PeerInfo {
    pub score: u32,
    /// virtual ms of the last charge (score decays 1 point a minute, `peers::Peer::score_now`)
    pub scored_at: u64,
    pub best_height: u64,
    pub best_work: [u8; 32],
}

#[derive(Clone, Debug)]
enum Pending {
    Status,
    Headers,
    /// ids requested for sync (`true`) or for one announced block (`false`)
    Blocks(Vec<(u64, Id)>, bool),
    Checkpoints,
}

/// How a node behaves on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Honest,
    /// serves only its public blocks, never relays others' headers or txs
    /// (the selfish / private-fork miner's node)
    Withholder,
    /// relays nothing and serves nothing (the eclipse attacker's sink)
    Silent,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub applied: u64,
    pub reorgs: Vec<(u64, u64)>, // (virtual ms, depth)
    pub deep_refusals: u64,
    pub invalid_blocks: u64,
    pub conservation_failures: Vec<String>,
    pub apply_us_max: u64,
    pub apply_us_sum: u64,
    pub apply_us: Vec<u64>,
    pub template_us_max: u64,
    /// mempool upkeep after a tip move (remove mined, re-admit everything)
    pub upkeep_us_max: u64,
    pub upkeep_us_sum: u64,
    pub mempool_max: usize,
    pub mempool_bytes_max: usize,
    pub charges: BTreeMap<String, u64>,
    pub bans: Vec<(u64, usize)>,
    pub ban_reasons: Vec<String>,
    pub held: u64,
    pub settle_waits: u64,
    pub settle_wait_ms: u64,
    pub last_tip_ms: u64,
    pub max_tip_gap_ms: u64,
    pub panics: Vec<String>,
}

pub struct Config {
    pub genesis: Arc<Vec<u8>>,
    /// hemera(genesis bytes), computed once (bostrom's file is 330 MB)
    pub chain_id: Id,
    pub opts: OpenOptions,
    pub clock: EpochClock,
    pub supply0: u64,
    pub subsidy: u64,
    /// unix seconds at virtual time 0 (the genesis time)
    pub t0_unix: u64,
}

pub struct SimNode {
    pub ix: usize,
    pub name: String,
    pub home: PathBuf,
    pub cfg: Arc<Config>,
    pub chain: Option<KernelChain>,
    events: Option<broadcast::Receiver<ChainEvent>>,
    pub mempool: Mempool,
    pub peers: BTreeMap<usize, PeerInfo>,
    pub banned: BTreeMap<usize, u64>,
    pending: BTreeMap<u64, (usize, Pending, u64)>,
    next_req: u64,
    recent: BTreeMap<Id, u64>,
    pub checkpoints: BTreeMap<u64, Checkpoint>,
    /// ignores everything it is sent but requests (a private-fork miner hiding)
    pub deaf: bool,
    /// the highest header height learned from others
    pub foreign_best: u64,
    /// gossip messages already delivered (iroh-gossip delivers a message to the
    /// application once; its message cache drops repeats): hash → virtual ms
    seen_gossip: BTreeMap<u64, u64>,
    /// blocks this node serves that its kernel never accepted (forged by an attacker)
    pub forged: BTreeMap<Id, (Header, Vec<u8>)>,
    pub skew_s: i64,
    pub role: Role,
    /// blocks this node mined and withholds (served to nobody)
    pub private: BTreeSet<Id>,
    pub ticks: u64,
    pub stats: Stats,
    pub outbox: Vec<(usize, Wire)>,
    pub tip_moved: bool,
    /// virtual ms of the settlement compute after a boundary (0 = instant)
    pub settle_cpu_ms: u64,
    /// boundary block id → virtual ms its settlement is ready
    settle_ready: BTreeMap<Id, u64>,
    /// work that waits for a settlement: (ready at, from, header, body)
    pub deferred: Vec<(u64, usize, Header, Vec<u8>)>,
    pub online: bool,
    pub now_ms: u64,
    /// chain-wire §3.2 buffer of fetched sync bodies, applied in height order
    sync_buffer: BTreeMap<u64, (Header, Vec<u8>, usize)>,
}

fn hex4(id: &Id) -> String {
    id[..4].iter().map(|b| format!("{b:02x}")).collect()
}

impl SimNode {
    pub fn new(ix: usize, name: String, home: PathBuf, cfg: Arc<Config>) -> SimNode {
        SimNode {
            ix,
            name,
            home,
            cfg,
            chain: None,
            events: None,
            mempool: Mempool::new(),
            peers: BTreeMap::new(),
            banned: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_req: 1,
            recent: BTreeMap::new(),
            checkpoints: BTreeMap::new(),
            deaf: false,
            foreign_best: 0,
            forged: BTreeMap::new(),
            seen_gossip: BTreeMap::new(),
            skew_s: 0,
            role: Role::Honest,
            private: BTreeSet::new(),
            ticks: 0,
            stats: Stats::default(),
            outbox: Vec::new(),
            tip_moved: false,
            settle_cpu_ms: 0,
            settle_ready: BTreeMap::new(),
            deferred: Vec::new(),
            online: false,
            now_ms: 0,
            sync_buffer: BTreeMap::new(),
        }
    }

    /// Open (or reopen after a crash) the store; a reopen replays the main chain.
    pub fn start(&mut self, now_ms: u64) -> Result<u64, String> {
        let t0 = Instant::now();
        let chain = KernelChain::open_with(&self.cfg.genesis, &self.home, self.cfg.opts.clone())?;
        self.events = Some(chain.events());
        self.chain = Some(chain);
        self.mempool = Mempool::new();
        self.pending.clear();
        self.sync_buffer.clear();
        self.deferred.clear();
        self.online = true;
        self.now_ms = now_ms;
        self.stats.last_tip_ms = now_ms;
        Ok(t0.elapsed().as_millis() as u64)
    }

    /// Crash: the process dies; the store stays on disk.
    pub fn crash(&mut self) {
        self.chain = None;
        self.events = None;
        self.mempool = Mempool::new();
        self.pending.clear();
        self.sync_buffer.clear();
        self.deferred.clear();
        self.peers.clear();
        self.online = false;
    }

    pub fn chain(&self) -> &KernelChain {
        self.chain.as_ref().expect("node is online")
    }

    /// The node's own clock, unix seconds.
    pub fn now(&self) -> u64 {
        ((self.cfg.t0_unix + self.now_ms / 1000) as i64 + self.skew_s).max(0) as u64
    }

    pub fn tip(&self) -> soft3::chain::net::chain_view::Tip {
        self.chain().tip()
    }

    // ── sending ─────────────────────────────────────────────────────

    pub fn gossip(&mut self, frame: &Frame, except: Option<usize>) {
        let bytes = frame.encode();
        let peers: Vec<usize> = self.peers.keys().copied().filter(|p| Some(*p) != except).collect();
        for p in peers {
            self.outbox.push((p, Wire::Gossip(bytes.clone())));
        }
    }

    fn request(&mut self, peer: usize, frame: Frame, what: Pending) -> u64 {
        let id = self.next_req;
        self.next_req += 1;
        self.pending.insert(id, (peer, what, self.now_ms));
        self.outbox.push((peer, Wire::Req(id, vec![frame.encode()])));
        id
    }

    fn charge(&mut self, peer: usize, points: u32, why: &str) {
        if points == 0 {
            return;
        }
        // the reason without its numbers: "tx: nonce 20 outside [3, 3 + 16)" → "tx: nonce outside"
        let key: String = why.split(|c: char| c.is_ascii_digit() || c == '[' || c == '(').next().unwrap_or(why).trim().to_string();
        *self.stats.charges.entry(key).or_default() += points as u64;
        let now = self.now_ms;
        let banned = match self.peers.get_mut(&peer) {
            Some(p) => {
                let decayed = p.score.saturating_sub((now.saturating_sub(p.scored_at) / 60_000) as u32);
                p.score = decayed.saturating_add(points);
                p.scored_at = now;
                p.score >= BAN_SCORE
            }
            None => false,
        };
        if banned {
            self.peers.remove(&peer);
            self.banned.insert(peer, self.now_ms + BAN_MS);
            self.stats.bans.push((self.now_ms, peer));
            self.stats.ban_reasons.push(why.to_string());
        }
    }

    fn note_relayed(&mut self, id: Id) {
        if self.recent.len() > 4096 {
            let floor = self.now_ms.saturating_sub(120_000);
            self.recent.retain(|_, t| *t >= floor);
        }
        self.recent.insert(id, self.now_ms);
    }

    fn recently_relayed(&self, id: &Id) -> bool {
        self.recent.get(id).is_some_and(|t| self.now_ms.saturating_sub(*t) < 120_000)
    }

    pub fn connected(&mut self, peer: usize) {
        if self.banned.get(&peer).is_some_and(|until| *until > self.now_ms) {
            return;
        }
        self.peers.entry(peer).or_default();
    }

    pub fn disconnected(&mut self, peer: usize) {
        self.peers.remove(&peer);
        self.pending.retain(|_, (p, _, _)| *p != peer);
    }

    // ── the kernel, with bookkeeping ────────────────────────────────

    /// The block at `h` would carry a settlement that is not ready yet.
    fn settlement_blocks(&self, h: u64) -> Option<u64> {
        if self.settle_cpu_ms == 0 {
            return None;
        }
        let e = self.cfg.clock.settles_at(h)?;
        let boundary = self.chain().header_at(self.cfg.clock.boundary(e))?.id();
        let ready = *self.settle_ready.get(&boundary)?;
        (ready > self.now_ms).then_some(ready)
    }

    /// When a miner on this node can build the block after the tip.
    pub fn template_ready_at(&self) -> Option<u64> {
        self.settlement_blocks(self.tip().height + 1)
    }

    pub fn accept_block(&mut self, from: usize, header: &Header, body: &[u8]) -> Result<BlockVerdict, BlockError> {
        if self.chain.as_ref().and_then(|c| c.header(&header.prev)).is_some() && self.settlement_blocks(header.height).is_some() {
            let parent_on_main = self.chain().height_of(&header.prev).is_some();
            if parent_on_main {
                let ready = self.settlement_blocks(header.height).expect("checked");
                self.stats.settle_waits += 1;
                self.deferred.push((ready, from, *header, body.to_vec()));
                return Err(BlockError::Held);
            }
        }
        let before = self.tip();
        let t0 = Instant::now();
        let now = self.now();
        let chain = self.chain.as_ref().expect("online");
        let verdict = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| chain.accept_block(header, body, now))) {
            Ok(v) => v,
            Err(p) => {
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                self.stats.panics.push(format!("accept_block {}: {msg}", header.height));
                self.crash();
                return Err(BlockError::Store("panicked".into()));
            }
        };
        let us = t0.elapsed().as_micros() as u64;
        if !matches!(verdict, Ok(BlockVerdict::Known)) {
            self.stats.apply_us_max = self.stats.apply_us_max.max(us);
            self.stats.apply_us_sum += us;
            self.stats.apply_us.push(us);
        }
        match &verdict {
            Err(BlockError::Refused(_)) => self.stats.deep_refusals += 1,
            Err(BlockError::Invalid(_)) => self.stats.invalid_blocks += 1,
            Err(BlockError::Held) => self.stats.held += 1,
            _ => {}
        }
        self.after_write(before);
        verdict
    }

    /// Drain the kernel's tip events: mempool upkeep, conservation, the
    /// settlement clock of every boundary that became main.
    fn after_write(&mut self, before: soft3::chain::net::chain_view::Tip) {
        let mut moved = false;
        let mut reorged: Vec<Vec<u8>> = Vec::new();
        let mut ids = Vec::new();
        if let Some(rx) = self.events.as_mut() {
            while let Ok(ev) = rx.try_recv() {
                let ChainEvent::NewTip { id, reorged_out, .. } = ev;
                moved = true;
                reorged.extend(reorged_out);
                ids.push(id);
            }
        }
        if !moved {
            return;
        }
        let chain = self.chain.as_ref().expect("online");
        let after = chain.tip();
        // reorg depth: blocks of the old main chain that left it
        if !reorged.is_empty() {
            self.stats.reorgs.push((self.now_ms, reorged.len() as u64));
        }
        let now = self.now();
        let t_upkeep = Instant::now();
        // mempool upkeep for every block that joined the main chain
        let mut fork = before.height.min(after.height);
        while fork > 0 && chain.header_at(fork).map(|h| h.id()) != self.ancestor_id(&before.id, fork) {
            fork -= 1;
        }
        for h in fork + 1..=after.height {
            if let Some((hd, body)) = chain.block_at(h) {
                self.mempool.remove_mined(chain, &body);
                if self.cfg.clock.is_boundary(h) {
                    self.settle_ready.entry(hd.id()).or_insert(self.now_ms + self.settle_cpu_ms);
                }
            }
        }
        self.mempool.revalidate(chain, &reorged, now);
        let up = t_upkeep.elapsed().as_micros() as u64;
        self.stats.upkeep_us_max = self.stats.upkeep_us_max.max(up);
        self.stats.upkeep_us_sum += up;
        self.stats.applied += after.height.saturating_sub(fork);
        if !chain.conserved() || chain.supply(&chain.native_token()) != self.cfg.supply0 + after.height * self.cfg.subsidy {
            self.stats.conservation_failures.push(format!("{} at height {} ({})", self.name, after.height, hex4(&after.id)));
        }
        self.stats.max_tip_gap_ms = self.stats.max_tip_gap_ms.max(self.now_ms - self.stats.last_tip_ms);
        self.stats.last_tip_ms = self.now_ms;
        self.tip_moved = true;
        let _ = ids;
    }

    fn ancestor_id(&self, tip: &Id, height: u64) -> Option<Id> {
        let chain = self.chain();
        let mut cur = chain.header(tip)?;
        if cur.height < height {
            return None;
        }
        while cur.height > height {
            cur = chain.header(&cur.prev)?;
        }
        Some(cur.id())
    }

    pub fn submit_tx(&mut self, bytes: &[u8]) -> Result<Id, AdmitError> {
        let now = self.now();
        let chain = self.chain.as_ref().expect("online");
        let r = self.mempool.admit(bytes, chain, now);
        self.stats.mempool_max = self.stats.mempool_max.max(self.mempool.len());
        self.stats.mempool_bytes_max = self.stats.mempool_bytes_max.max(self.mempool.bytes());
        if r.is_ok() && self.role == Role::Honest {
            let id = txid(&chain.chain_id(), bytes);
            self.note_relayed(id);
            self.gossip(&Frame::Tx(bytes.to_vec()), None);
        }
        r
    }

    /// Announce a block this node mined (and holds): header gossip.
    pub fn announce(&mut self, header: &Header) {
        self.note_relayed(header.id());
        self.gossip(&Frame::HeaderGossip(*header), None);
    }

    pub fn publish_checkpoint(&mut self, cp: Checkpoint) {
        let _ = self.chain().accept_checkpoint(cp.height, &cp.block_id, &cp.sig);
        self.checkpoints.insert(cp.height, cp.clone());
        self.gossip(&Frame::Checkpoint(cp), None);
    }

    // ── receiving ───────────────────────────────────────────────────

    fn trace(&self, what: &str) {
        if std::env::var("SIM_TRACE").is_ok_and(|v| v == self.name) {
            let t = self.chain.as_ref().map(|c| c.tip().height).unwrap_or(0);
            eprintln!("    {} t={}s tip={t}: {what}", self.name, self.now_ms / 1000);
        }
    }

    pub fn on_wire(&mut self, from: usize, wire: Wire) {
        if !self.online {
            return;
        }
        if std::env::var("SIM_TRACE").is_ok_and(|v| v == self.name) {
            let kind = match &wire {
                Wire::Gossip(b) => format!("gossip {}B", b.len()),
                Wire::Req(id, _) => format!("req {id}"),
                Wire::Resp(id, f) => format!("resp {id} ({} frames)", f.len()),
            };
            self.trace(&format!("from {from}: {kind}"));
        }
        if !self.peers.contains_key(&from) {
            // a message from a peer we dropped (ban, eclipse): ignore
            if !matches!(wire, Wire::Resp(..)) {
                return;
            }
        }
        match wire {
            Wire::Gossip(bytes) => self.on_gossip(from, &bytes),
            Wire::Req(id, frames) => self.on_request(from, id, &frames),
            Wire::Resp(id, frames) => self.on_response(from, id, &frames),
        }
    }

    fn decode_gossip(&mut self, from: usize, bytes: &[u8]) -> Option<Frame> {
        match Frame::decode(bytes, MAX_GOSSIP_BYTES) {
            Ok(f) => Some(f),
            Err(FrameError::Oversize(_)) => {
                self.charge(from, 100, "gossip frame over size");
                None
            }
            Err(e) => {
                self.charge(from, 10, &format!("gossip frame: {e}"));
                None
            }
        }
    }

    fn on_gossip(&mut self, from: usize, bytes: &[u8]) {
        // iroh-gossip's message cache: a repeat of a delivered message never
        // reaches the application (a fixed-key SipHash, deterministic)
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut h);
            h.finish()
        };
        if self.seen_gossip.contains_key(&key) {
            return;
        }
        if self.seen_gossip.len() > 65_536 {
            let floor = self.now_ms.saturating_sub(30 * 60_000);
            self.seen_gossip.retain(|_, t| *t >= floor);
        }
        self.seen_gossip.insert(key, self.now_ms);
        match self.decode_gossip(from, bytes) {
            Some(Frame::HeaderGossip(h)) => self.on_header(from, h),
            Some(Frame::Tx(tx)) => self.on_tx(from, tx),
            Some(Frame::Checkpoint(c)) => self.on_checkpoint(from, c),
            Some(Frame::Block { .. }) => self.charge(from, 100, "body on gossip"),
            Some(_) => self.charge(from, 10, "wrong frame on a topic"),
            None => {}
        }
    }

    fn on_header(&mut self, from: usize, header: Header) {
        let id = header.id();
        let now = self.now();
        let verdict = self.chain().accept_header(&header, now);
        self.trace(&format!("header {} from {from}: {verdict:?}", header.height));
        match verdict {
            Ok(HeaderVerdict::Accepted) => {
                self.foreign_best = self.foreign_best.max(header.height);
                if self.role == Role::Honest {
                    self.note_relayed(id);
                    self.gossip(&Frame::HeaderGossip(header), Some(from));
                }
                if let Some(p) = self.peers.get_mut(&from) {
                    if header.height > p.best_height {
                        p.best_height = header.height;
                    }
                }
                if self.chain().has_body(&header.prev) {
                    self.request(from, Frame::GetBlocks(vec![id]), Pending::Blocks(vec![(header.height, id)], false));
                } else {
                    self.sync_pass(Some(from));
                }
            }
            Ok(HeaderVerdict::Known) => {
                if !self.recently_relayed(&id) {
                    self.charge(from, 1, "duplicate header");
                }
            }
            Ok(HeaderVerdict::Orphan) => self.sync_pass(Some(from)),
            Ok(HeaderVerdict::Held) => self.stats.held += 1,
            Err(e) => self.charge(from, e.score(), &format!("header: {e}")),
        }
    }

    fn on_tx(&mut self, from: usize, bytes: Vec<u8>) {
        let id = txid(&self.chain().chain_id(), &bytes);
        let now = self.now();
        let chain = self.chain.as_ref().expect("online");
        let r = self.mempool.admit(&bytes, chain, now);
        self.stats.mempool_max = self.stats.mempool_max.max(self.mempool.len());
        self.stats.mempool_bytes_max = self.stats.mempool_bytes_max.max(self.mempool.bytes());
        match r {
            Ok(_) => {
                self.note_relayed(id);
                if self.role == Role::Honest {
                    self.gossip(&Frame::Tx(bytes), Some(from));
                }
            }
            Err(AdmitError::Known) if self.recently_relayed(&id) => {}
            Err(e) => {
                // the wire's own rule (`net::mempool::relay_charge`, chain-wire §5)
                let peer_height = self.peers.get(&from).map(|p| p.best_height).unwrap_or(0);
                let points = soft3::chain::net::mempool::relay_charge(&e, peer_height, self.tip().height);
                self.charge(from, points, &format!("tx: {e}"));
            }
        }
    }

    fn on_checkpoint(&mut self, from: usize, c: Checkpoint) {
        if self.chain().checkpoint_key().is_none() || self.checkpoints.contains_key(&c.height) {
            return;
        }
        match self.chain().accept_checkpoint(c.height, &c.block_id, &c.sig) {
            Ok(_) => {
                self.checkpoints.insert(c.height, c.clone());
                self.tip_moved = true;
                if self.role == Role::Honest {
                    self.gossip(&Frame::Checkpoint(c), Some(from));
                }
            }
            Err(e) if e.contains("signature") => self.charge(from, 10, "checkpoint signature"),
            Err(_) => {
                // a valid signature the kernel refuses (sunset, conflict): held anyway by the wire
                self.checkpoints.insert(c.height, c.clone());
                if self.role == Role::Honest {
                    self.gossip(&Frame::Checkpoint(c), Some(from));
                }
            }
        }
    }

    fn public(&self, id: &Id) -> bool {
        !self.private.contains(id)
    }

    fn on_request(&mut self, from: usize, id: u64, frames: &[Vec<u8>]) {
        if self.role == Role::Silent {
            return;
        }
        let Some(raw) = frames.first() else { return };
        let request = match Frame::decode(raw, frames::MAX_REQUEST_BYTES) {
            Ok(f) => f,
            Err(e) => {
                self.charge(from, 10, &format!("request: {e}"));
                return;
            }
        };
        let chain = self.chain.as_ref().expect("online");
        let mut out: Vec<Frame> = Vec::new();
        match request {
            Frame::Status(s) => {
                if s.chain != chain.chain_id() || s.genesis != chain.genesis_id() {
                    self.charge(from, 100, "wrong chain");
                    return;
                }
                if let Some(p) = self.peers.get_mut(&from) {
                    p.best_height = s.height;
                    p.best_work = s.chainwork;
                }
                out.push(self.status_frame());
            }
            Frame::GetHeaders { locator, stop } => {
                let mut hs = chain.headers_from_locator(&locator, &stop, MAX_HEADERS_PER_RESPONSE);
                if let Some(cut) = hs.iter().position(|h| !self.public(&h.id())) {
                    hs.truncate(cut);
                }
                out.push(Frame::Headers(hs));
            }
            Frame::GetBlocks(ids) => {
                if ids.len() > MAX_BLOCKS_BY_ID {
                    out.push(Frame::Error { code: frames::ERR_LIMIT, reason: "too many ids".into() });
                } else {
                    for i in ids {
                        let forged = self.forged.get(&i).cloned();
                        match forged.or_else(|| chain.block(&i).filter(|_| self.public(&i))) {
                            Some((header, body)) => out.push(Frame::Block { header, body }),
                            None => {
                                out.push(Frame::Error { code: frames::ERR_UNKNOWN, reason: "unknown block".into() });
                                break;
                            }
                        }
                    }
                }
            }
            Frame::GetCheckpoints { from_height } => {
                for c in self.checkpoints.range(from_height..).map(|(_, c)| c.clone()) {
                    out.push(Frame::Checkpoint(c));
                }
            }
            _ => out.push(Frame::Error { code: frames::ERR_UNKNOWN, reason: "not a request".into() }),
        }
        out.push(Frame::End);
        self.outbox.push((from, Wire::Resp(id, out.iter().map(|f| f.encode()).collect())));
    }

    pub fn status_frame(&self) -> Frame {
        let chain = self.chain();
        // a withholder reports its best public block
        let mut tip = chain.tip();
        if !self.private.is_empty() {
            let mut h = tip.height;
            while h > 0 {
                let id = chain.header_at(h).map(|x| x.id()).unwrap_or([0; 32]);
                if self.public(&id) {
                    break;
                }
                h -= 1;
            }
            let id = chain.header_at(h).map(|x| x.id()).unwrap_or(tip.id);
            tip = soft3::chain::net::chain_view::Tip { height: h, id, chainwork: chain.chainwork_of(&id).unwrap_or(tip.chainwork) };
        }
        Frame::Status(frames::Status { chain: chain.chain_id(), rule_set: chain.rule_set(), height: tip.height, tip: tip.id, chainwork: tip.chainwork.to_le_bytes(), genesis: chain.genesis_id() })
    }

    fn on_response(&mut self, from: usize, id: u64, raw: &[Vec<u8>]) {
        let Some((peer, what, _)) = self.pending.remove(&id) else { return };
        if peer != from {
            return;
        }
        let mut frames = Vec::new();
        for r in raw {
            match Frame::decode(r, frames::MAX_RESPONSE_BYTES) {
                Ok(f) => frames.push(f),
                Err(e) => {
                    self.charge(from, 10, &format!("response: {e}"));
                    return;
                }
            }
        }
        match what {
            Pending::Status => {
                if let Some(Frame::Status(s)) = frames.first() {
                    let chain = self.chain();
                    if s.chain != chain.chain_id() || s.genesis != chain.genesis_id() {
                        self.charge(from, 100, "wrong chain");
                        return;
                    }
                    if let Some(p) = self.peers.get_mut(&from) {
                        p.best_height = s.height;
                        p.best_work = s.chainwork;
                    }
                    let theirs = U256::from_le_bytes(&s.chainwork);
                    if theirs > self.chain().best_header().chainwork || self.chain().header(&s.tip).is_none() {
                        let locator = self.best_locator();
                        self.request(from, Frame::GetHeaders { locator, stop: [0; 32] }, Pending::Headers);
                    } else {
                        self.body_sync();
                    }
                }
            }
            Pending::Headers => {
                let Some(Frame::Headers(hs)) = frames.first() else { return };
                let now = self.now();
                let mut accepted = 0;
                let mut stop = false;
                for h in hs {
                    match self.chain().accept_header(h, now) {
                        Ok(HeaderVerdict::Accepted) => accepted += 1,
                        Ok(HeaderVerdict::Known) => {}
                        Ok(HeaderVerdict::Orphan) => {
                            stop = true;
                            break;
                        }
                        Ok(HeaderVerdict::Held) => {
                            stop = true;
                            break;
                        }
                        Err(e) => {
                            self.charge(from, e.score(), &format!("header at {}: {e}", h.height));
                            stop = true;
                            break;
                        }
                    }
                }
                let _ = accepted;
                if !stop && hs.len() >= MAX_HEADERS_PER_RESPONSE {
                    let locator = self.best_locator();
                    self.request(from, Frame::GetHeaders { locator, stop: [0; 32] }, Pending::Headers);
                }
                self.body_sync();
            }
            Pending::Blocks(want, sync) => {
                let mut got = Vec::new();
                for ((height, id), f) in want.iter().zip(frames) {
                    match f {
                        Frame::Block { header, body } if header.id() == *id => got.push((*height, header, body)),
                        Frame::Block { .. } => {
                            self.charge(from, 100, "body for a header we did not request");
                            return;
                        }
                        _ => break,
                    }
                }
                if !sync && got.is_empty() {
                    // the announcer relayed before its own body arrived (gossip::fetch_body): sync takes over
                    self.sync_pass(Some(from));
                }
                if sync {
                    for (h, header, body) in got {
                        self.sync_buffer.insert(h, (header, body, from));
                    }
                    self.drain_sync_buffer();
                } else {
                    for (_, header, body) in got {
                        self.apply_fetched(from, header, body);
                    }
                }
            }
            Pending::Checkpoints => {
                for f in frames {
                    if let Frame::Checkpoint(c) = f {
                        self.on_checkpoint(from, c);
                    }
                }
            }
        }
    }

    /// `gossip::apply_fetched`: a block from its announcer; ancestors on demand.
    fn apply_fetched(&mut self, from: usize, header: Header, body: Vec<u8>) {
        let v = self.accept_block(from, &header, &body);
        self.trace(&format!("block {} from {from}: {v:?}", header.height));
        match v {
            Ok(BlockVerdict::NewTip) | Ok(BlockVerdict::Stored) | Ok(BlockVerdict::Known) => {}
            Ok(BlockVerdict::NeedBodies(ids)) => {
                let want: Vec<(u64, Id)> = ids
                    .iter()
                    .take(MAX_RANGE as usize)
                    .map(|i| (self.chain().header(i).map(|h| h.height).unwrap_or(0), *i))
                    .chain(std::iter::once((header.height, header.id())))
                    .collect();
                let ids: Vec<Id> = want.iter().map(|w| w.1).collect();
                self.request(from, Frame::GetBlocks(ids), Pending::Blocks(want, true));
            }
            Err(BlockError::UnknownParent) => self.sync_pass(Some(from)),
            Err(BlockError::Held | BlockError::Refused(_) | BlockError::Store(_)) => {}
            Err(e) => self.charge(from, e.score(), &format!("block {}: {e}", header.height)),
        }
    }

    fn drain_sync_buffer(&mut self) {
        let buffer = std::mem::take(&mut self.sync_buffer);
        for (height, (header, body, peer)) in buffer {
            if !self.online {
                return;
            }
            match self.accept_block(peer, &header, &body) {
                Ok(_) => {}
                Err(e @ (BlockError::Held | BlockError::Refused(_) | BlockError::Store(_) | BlockError::UnknownParent)) => {
                    let _ = (height, e);
                    break;
                }
                Err(e) => {
                    self.charge(peer, e.score(), &format!("block {height}: {e}"));
                    break;
                }
            }
        }
        if self.online {
            self.body_sync();
        }
    }

    /// `sync::best_locator`: the locator of the best header chain.
    fn best_locator(&self) -> Vec<Id> {
        let chain = self.chain();
        let best = chain.best_header();
        let mut out = Vec::new();
        let mut id = best.id;
        let mut step = 1u64;
        let mut height = best.height;
        loop {
            out.push(id);
            if height == 0 || out.len() >= 255 {
                break;
            }
            if out.len() >= 10 {
                step *= 2;
            }
            let target = height.saturating_sub(step);
            let mut cursor = id;
            let mut h = height;
            while h > target {
                match chain.header(&cursor) {
                    Some(hd) => {
                        cursor = hd.prev;
                        h -= 1;
                    }
                    None => return out,
                }
            }
            id = cursor;
            height = target;
        }
        out
    }

    fn sync_in_flight(&self) -> usize {
        self.pending.values().filter(|(_, w, _)| matches!(w, Pending::Blocks(_, true))).count()
    }

    /// `sync::body_sync`: bodies of the best header chain above the tip, in ranges.
    fn body_sync(&mut self) {
        if self.sync_in_flight() > 0 {
            return;
        }
        let missing = self.chain().missing_bodies(soft3::chain::net::sync::BUFFER_BLOCKS);
        if missing.is_empty() {
            return;
        }
        let top = missing.last().map(|m| m.0).unwrap_or(0);
        let mut peers: Vec<usize> = self.peers.iter().filter(|(_, p)| p.best_height >= top).map(|(i, _)| *i).collect();
        if peers.is_empty() {
            peers = self.peers.keys().copied().collect();
        }
        if peers.is_empty() {
            return;
        }
        let ranges: Vec<Vec<(u64, Id)>> = missing.chunks(MAX_RANGE as usize).take(soft3::chain::net::sync::RANGES_IN_FLIGHT).map(|c| c.to_vec()).collect();
        let offset = (self.ticks as usize) % peers.len();
        for (i, range) in ranges.into_iter().enumerate() {
            let peer = peers[(i + offset) % peers.len()];
            let ids: Vec<Id> = range.iter().map(|r| r.1).collect();
            self.request(peer, Frame::GetBlocks(ids), Pending::Blocks(range, true));
        }
    }

    /// `sync::pass`: held headers, status of the peers ahead (all, every third tick), bodies.
    pub fn sync_pass(&mut self, preferred: Option<usize>) {
        if !self.online {
            return;
        }
        let now = self.now();
        for h in self.chain().retry_held(now) {
            if self.role == Role::Honest {
                self.note_relayed(h.id());
                self.gossip(&Frame::HeaderGossip(h), None);
            }
        }
        let refresh = preferred.is_none() && self.ticks % 3 == 0;
        let ours = self.tip();
        let mut candidates: Vec<(U256, usize)> = self
            .peers
            .iter()
            .filter(|(_, p)| refresh || U256::from_le_bytes(&p.best_work) > ours.chainwork || p.best_height > ours.height)
            .map(|(i, p)| (U256::from_le_bytes(&p.best_work), *i))
            .collect();
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        if let Some(p) = preferred {
            candidates.retain(|(_, i)| *i != p);
            candidates.insert(0, (U256::ZERO, p));
        }
        let limit = if refresh { 8 } else { 2 };
        let busy: BTreeSet<usize> = self.pending.values().filter(|(_, w, _)| matches!(w, Pending::Status | Pending::Headers)).map(|(p, _, _)| *p).collect();
        let status = self.status_frame();
        for (_, peer) in candidates.into_iter().take(limit) {
            if !busy.contains(&peer) {
                self.request(peer, status.clone(), Pending::Status);
            }
        }
        self.body_sync();
    }

    /// The 5 s tick: expire requests, re-run deferred work, sync.
    pub fn tick(&mut self) {
        if !self.online {
            return;
        }
        self.ticks += 1;
        let now = self.now_ms;
        self.pending.retain(|_, (_, _, sent)| now.saturating_sub(*sent) < REQUEST_TIMEOUT_MS);
        self.banned.retain(|_, until| *until > now);
        self.wake();
        self.sync_pass(None);
    }

    /// Re-run blocks deferred for a settlement that is ready now.
    pub fn wake(&mut self) {
        let now = self.now_ms;
        let (ready, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.deferred).into_iter().partition(|(t, ..)| *t <= now);
        self.deferred = later;
        for (t, from, header, body) in ready {
            self.stats.settle_wait_ms = self.stats.settle_wait_ms.max(now.saturating_sub(t));
            self.apply_fetched(from, header, body);
        }
    }

    pub fn ask_checkpoints(&mut self) {
        let peers: Vec<usize> = self.peers.keys().copied().collect();
        for p in peers {
            self.request(p, Frame::GetCheckpoints { from_height: 0 }, Pending::Checkpoints);
        }
    }

    /// Main-chain ids `[from, to]` (for safety comparisons).
    pub fn main_ids(&self, from: u64, to: u64) -> Vec<Id> {
        let chain = self.chain();
        (from..=to).filter_map(|h| chain.header_at(h).map(|x| x.id())).collect()
    }

    pub fn finality_floor(&self) -> u64 {
        self.tip().height.saturating_sub(FINALITY_DEPTH)
    }

    pub fn short(&self) -> String {
        let t = self.tip();
        format!("{}@{}:{}", self.name, t.height, hex4(&t.id))
    }
}
