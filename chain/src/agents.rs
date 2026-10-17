//! Agents: honest transaction traffic, the flood, junk and replays from
//! hostile peers, forged blocks with valid work, and the economy's
//! strategies. Every transaction is a real signed rule-set-1 transaction;
//! hostile peers are simulated nodes whose frames reach honest nodes over
//! the simulated links and are scored by the honest nodes' own rules.

use std::collections::BTreeMap;

use soft3::chain::net::chain_view::ChainView;
use soft3::chain::net::codec::Header;
use soft3::chain::net::frames::Frame;
use soft3::chain::sigs;
use soft3::chain::tx::{Body, Tx};
use soft3::wallet::tx::card_id;

use crate::genesis::{Account, Legacy};
use crate::node::Role;
use crate::rng::Rng;
use crate::world::{Agent, World, MIN, SEC};

pub type Id = [u8; 32];

pub fn signed(chain: &Id, body: Body, key: &mudra::SigningKey) -> Vec<u8> {
    let mut tx = Tx { body, sig: vec![] };
    sigs::sign(chain, &mut tx, key).expect("sign");
    tx.encode()
}

pub fn chain_id(w: &World) -> Id {
    w.cfg.chain_id
}

pub fn native() -> Id {
    sigs::token("pussy")
}

fn online_honest(w: &World, among: &[usize]) -> Vec<usize> {
    among.iter().copied().filter(|n| w.nodes[*n].online && w.nodes[*n].role == Role::Honest).collect()
}

/// The next nonce of `neuron` at `node` (chain nonce advanced past its mempool).
pub fn next_nonce(w: &World, node: usize, neuron: &Id) -> u64 {
    let n = &w.nodes[node];
    n.mempool.next_nonce(n.chain(), neuron)
}

pub fn submit(w: &mut World, node: usize, tx: &[u8]) -> bool {
    let ok = w.nodes[node].submit_tx(tx).is_ok();
    w.flush(node);
    ok
}

// ── honest traffic ─────────────────────────────────────────────────

/// Transfers and zero-stake links from funded accounts, at a steady rate;
/// at the end, how many were mined and how long they waited.
pub struct Traffic {
    pub accounts: Vec<Account>,
    pub nodes: Vec<usize>,
    pub every_ms: u64,
    pub fee_bonus: u64,
    pub rng: Rng,
    pub sent: BTreeMap<Id, u64>,
    pub refused: u64,
    pub label: String,
    pub stop_at: u64,
    /// the share of submitted transactions that must be mined, percent
    pub min_share: u64,
}

impl Agent for Traffic {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn act(&mut self, w: &mut World) -> Option<u64> {
        if w.now >= self.stop_at {
            return None;
        }
        let nodes = online_honest(w, &self.nodes);
        if nodes.is_empty() || self.accounts.is_empty() {
            return Some(self.every_ms);
        }
        // a wallet talks to one node (its home); another only while the home is down
        let k = self.rng.below(self.accounts.len() as u64) as usize;
        let home = self.nodes[k % self.nodes.len()];
        let node = if nodes.contains(&home) { home } else { nodes[self.rng.below(nodes.len() as u64) as usize] };
        let a = &self.accounts[k];
        let nonce = next_nonce(w, node, &a.neuron);
        let chain = chain_id(w);
        let body = if self.rng.below(3) == 0 {
            let p = crate::genesis::particle(self.rng.below(5_000));
            let q = crate::genesis::particle(5_000 + self.rng.below(5_000));
            Body::Cyberlink { from: a.neuron, nonce, token: native(), amount: 0, p, q, valence: 0, fee: 257 + self.fee_bonus + self.rng.below(40) }
        } else {
            let to = self.accounts[self.rng.below(self.accounts.len() as u64) as usize].neuron;
            Body::Transfer { from: a.neuron, nonce, token: native(), amount: 1 + self.rng.below(1_000), to, fee: 224 + self.fee_bonus + self.rng.below(40) }
        };
        let tx = signed(&chain, body, &a.key);
        let id = soft3::chain::net::codec::txid(&chain, &tx);
        if submit(w, node, &tx) {
            self.sent.insert(id, w.now);
        } else {
            self.refused += 1;
        }
        Some(self.rng.exp(self.every_ms).max(1))
    }

    fn finish(&mut self, w: &mut World) {
        let Some(&obs) = w.honest_online().first() else { return };
        let mut waits = Vec::new();
        let mut mined = 0u64;
        for (id, t) in &self.sent {
            if let Some((h, _, _)) = w.nodes[obs].chain().tx_location(id) {
                mined += 1;
                let hd = w.nodes[obs].chain().header_at(h).expect("main");
                let found = w.found.get(&hd.id()).map(|f| f.0).unwrap_or(t + 0);
                waits.push(found.saturating_sub(*t));
            }
        }
        waits.sort_unstable();
        let pct = |p: usize| waits.get(waits.len().saturating_sub(1) * p / 100).copied().unwrap_or(0) / 1000;
        let sent = self.sent.len() as u64;
        let share = if sent == 0 { 100 } else { mined * 100 / sent };
        let ev = format!("{mined}/{sent} mined ({share} %), wait p50 {} s, p90 {} s, max {} s; {} refused at submit", pct(50), pct(90), pct(100), self.refused);
        w.metrics.emit(serde_json::json!({"kind": "traffic", "label": self.label, "sent": sent, "mined": mined, "p50_s": pct(50), "p90_s": pct(90), "max_s": pct(100)}));
        let inv = format!("{}: honest txs mined", self.label);
        w.checks.expect(&inv, sent == 0 || share >= self.min_share, format!("{ev} (bound ≥ {} %)", self.min_share));
    }
}

// ── the flood ──────────────────────────────────────────────────────

/// Valid transactions at the fee floor from many accounts, as fast as the
/// nonce window allows, all into one node; plus below-floor fee spam.
pub struct Flood {
    pub accounts: Vec<Account>,
    pub node: usize,
    pub per_round: usize,
    pub every_ms: u64,
    pub until: u64,
    pub rng: Rng,
    pub admitted: u64,
    pub refused: u64,
    pub below_floor_admitted: u64,
}

impl Agent for Flood {
    fn name(&self) -> String {
        "flood".into()
    }

    fn act(&mut self, w: &mut World) -> Option<u64> {
        if w.now >= self.until {
            return None;
        }
        if !w.nodes[self.node].online {
            return Some(self.every_ms);
        }
        let chain = chain_id(w);
        for _ in 0..self.per_round {
            let a = self.accounts[self.rng.below(self.accounts.len() as u64) as usize].clone();
            let nonce = next_nonce(w, self.node, &a.neuron);
            let cheap = self.rng.below(10) == 0;
            let fee = if cheap { 100 } else { 224 };
            let to = self.accounts[self.rng.below(self.accounts.len() as u64) as usize].neuron;
            let tx = signed(&chain, Body::Transfer { from: a.neuron, nonce, token: native(), amount: 1, to, fee }, &a.key);
            let ok = w.nodes[self.node].submit_tx(&tx).is_ok();
            if ok {
                self.admitted += 1;
                if cheap {
                    self.below_floor_admitted += 1;
                }
            } else {
                self.refused += 1;
            }
        }
        w.flush(self.node);
        Some(self.every_ms)
    }

    fn finish(&mut self, w: &mut World) {
        w.checks.expect("fee floor: below-floor txs refused", self.below_floor_admitted == 0, format!("{} below-floor admitted; {} flood txs admitted, {} refused", self.below_floor_admitted, self.admitted, self.refused));
        let max = w.nodes.iter().map(|n| n.stats.mempool_max).max().unwrap_or(0);
        let bytes = w.nodes.iter().map(|n| n.stats.mempool_bytes_max).max().unwrap_or(0);
        w.checks.expect(
            "mempool within capacity",
            max <= soft3::chain::net::mempool::CAPACITY_TXS && bytes <= soft3::chain::net::mempool::CAPACITY_BYTES,
            format!("max {max} txs, {} KB across nodes (cap {} txs)", bytes / 1024, soft3::chain::net::mempool::CAPACITY_TXS),
        );
    }
}

// ── hostile peers: junk, replays, claim abuse ──────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Junk {
    RandomBytes,
    OversizeGossip,
    BadSignature,
    OversizeTx,
    Replay,
    CrossChain,
    ClaimWrongKey,
    DoubleClaim,
    BodyOnGossip,
    BadPowHeader,
    WrongChainStatus,
}

impl Junk {
    pub const ALL: [Junk; 11] = [
        Junk::RandomBytes,
        Junk::OversizeGossip,
        Junk::BadSignature,
        Junk::OversizeTx,
        Junk::Replay,
        Junk::CrossChain,
        Junk::ClaimWrongKey,
        Junk::DoubleClaim,
        Junk::BodyOnGossip,
        Junk::BadPowHeader,
        Junk::WrongChainStatus,
    ];
}

/// One hostile peer (its own simulated node, linked to honest nodes) that
/// sends one kind of junk repeatedly.
pub struct Hostile {
    pub node: usize,
    pub kind: Junk,
    pub sent: u64,
    pub max: u64,
    pub rng: Rng,
    pub donor: Account,
    pub legacy: Option<Legacy>,
}

impl Agent for Hostile {
    fn name(&self) -> String {
        format!("hostile {:?}", self.kind)
    }

    fn act(&mut self, w: &mut World) -> Option<u64> {
        if self.sent >= self.max || !w.nodes[self.node].online {
            return None;
        }
        self.sent += 1;
        let chain = chain_id(w);
        let neighbors = w.net.neighbors(self.node);
        let mut gossip: Option<Vec<u8>> = None;
        match self.kind {
            Junk::RandomBytes => {
                let n = 20 + self.rng.below(200) as usize;
                gossip = Some((0..n).map(|_| self.rng.next() as u8).collect());
            }
            Junk::OversizeGossip => gossip = Some(Frame::Tx(vec![7u8; 5_000]).encode()),
            Junk::BadSignature => {
                let nonce = w.nodes[self.node].chain().nonce(&self.donor.neuron);
                let mut tx = signed(&chain, Body::Transfer { from: self.donor.neuron, nonce, token: native(), amount: 1, to: [3; 32], fee: 300 }, &self.donor.key);
                let i = tx.len() - 5;
                tx[i] ^= 0x40;
                gossip = Some(Frame::Tx(tx).encode());
            }
            Junk::OversizeTx => gossip = Some(Frame::Tx(vec![1u8; 600]).encode()),
            Junk::Replay => {
                // a transaction already on the main chain
                let n = &w.nodes[self.node];
                let tip = n.tip().height;
                let mut found = None;
                for h in (1..=tip).rev().take(50) {
                    if let Some((_, body)) = n.chain().block_at(h) {
                        if let Some(tx) = n.chain().txs_of(&body).into_iter().nth(1) {
                            found = Some(tx);
                            break;
                        }
                    }
                }
                gossip = found.map(|tx| Frame::Tx(tx).encode());
            }
            Junk::CrossChain => {
                let other = [0xee; 32];
                let nonce = w.nodes[self.node].chain().nonce(&self.donor.neuron);
                let tx = signed(&other, Body::Transfer { from: self.donor.neuron, nonce, token: native(), amount: 1, to: [3; 32], fee: 300 }, &self.donor.key);
                gossip = Some(Frame::Tx(tx).encode());
            }
            Junk::ClaimWrongKey => {
                if let Some(l) = &self.legacy {
                    // the donor's key claiming a legacy account it does not own
                    let tx = signed(&chain, Body::Claim { account_id: l.account_id, nonce: 0, fee: 300 }, &self.donor.key);
                    gossip = Some(Frame::Tx(tx).encode());
                }
            }
            Junk::DoubleClaim => {
                if let Some(l) = &self.legacy {
                    // the owner's real key; the first claim is valid, every later one has nothing to move
                    let neuron = sigs::neuron_of(&crate::genesis::pubkey(&l.key));
                    let nonce = w.nodes[self.node].chain().nonce(&neuron);
                    let tx = signed(&chain, Body::Claim { account_id: l.account_id, nonce, fee: 300 }, &l.key);
                    gossip = Some(Frame::Tx(tx).encode());
                }
            }
            Junk::BodyOnGossip => {
                let n = &w.nodes[self.node];
                if let Some((header, body)) = n.chain().block_at(n.tip().height) {
                    gossip = Some(Frame::Block { header, body }.encode());
                }
            }
            Junk::BadPowHeader => {
                let n = &w.nodes[self.node];
                if let Some(mut h) = n.chain().header_at(n.tip().height) {
                    h.prev = h.id();
                    h.height += 1;
                    h.time += 30;
                    h.bits = n.chain().next_bits();
                    // a nonce that fails the target
                    for k in 0u128.. {
                        h.nonce = k.to_le_bytes();
                        if !h.valid_pow() {
                            break;
                        }
                    }
                    gossip = Some(Frame::HeaderGossip(h).encode());
                }
            }
            Junk::WrongChainStatus => {
                let mut status = match w.nodes[self.node].status_frame() {
                    Frame::Status(s) => s,
                    _ => unreachable!(),
                };
                status.chain = [0xab; 32];
                for p in neighbors.iter().copied() {
                    w.nodes[self.node].outbox.push((p, crate::net::Wire::Req(9_000_000 + self.sent, vec![Frame::Status(status).encode()])));
                }
            }
        }
        if let Some(g) = gossip {
            for p in neighbors {
                w.nodes[self.node].outbox.push((p, crate::net::Wire::Gossip(g.clone())));
            }
        }
        w.flush(self.node);
        Some(10 * SEC + self.rng.below(10 * SEC))
    }

    fn finish(&mut self, w: &mut World) {
        let targets = w.net.all_links_of(self.node);
        let banned: Vec<usize> = targets.iter().copied().filter(|t| w.nodes[*t].stats.bans.iter().any(|(_, p)| *p == self.node)).collect();
        // what each kind must cost the sender (chain-wire §5): replays of mined
        // transactions are stale (refused, never scored — honest relays race
        // blocks); everything else bans
        let stale = matches!(self.kind, Junk::Replay | Junk::DoubleClaim);
        let ok = if stale { banned.is_empty() } else { banned.len() == targets.len() };
        let rule = if stale { "refused, not scored" } else { "banned by every target" };
        w.checks.expect(
            &format!("hostile {:?}: {rule}", self.kind),
            ok,
            format!("{} messages sent; banned by {}/{} targets", self.sent, banned.len(), targets.len()),
        );
    }
}

// ── forged blocks with valid work ──────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forge {
    Oversize,
    TxRoot,
    StateRoot,
    CoinbaseAmount,
    BadSigTx,
    DuplicateTx,
    TooManyTxs,
    FutureTime,
    WrongBits,
}

impl Forge {
    pub const ALL: [Forge; 9] =
        [Forge::Oversize, Forge::TxRoot, Forge::StateRoot, Forge::CoinbaseAmount, Forge::BadSigTx, Forge::DuplicateTx, Forge::TooManyTxs, Forge::FutureTime, Forge::WrongBits];
}

/// A hostile miner that builds an honest template on its node's tip,
/// breaks one rule, grinds valid work for the broken header, serves the
/// body and announces it. The honest nodes must never extend it.
pub struct Forger {
    pub node: usize,
    pub kinds: Vec<Forge>,
    pub next: usize,
    pub donor: Account,
    pub forged: Vec<(Forge, Id, u64)>,
    pub rng: Rng,
}

impl Forger {
    fn build(&mut self, w: &mut World, kind: Forge) -> Option<(Header, Vec<u8>)> {
        let chain = chain_id(w);
        let n = &w.nodes[self.node];
        let now = n.now();
        let nonce = n.chain().nonce(&self.donor.neuron);
        let good = signed(&chain, Body::Transfer { from: self.donor.neuron, nonce, token: native(), amount: 1, to: [9; 32], fee: 300 }, &self.donor.key);
        let (mut header, body) = n.chain().template(&[0x77; 32], &[good.clone()], now).ok()?;
        let mut txs = soft3::chain::tx::split_body(&body).ok()?;
        let reroot = |h: &mut Header, txs: &[Vec<u8>]| {
            h.tx_root = soft3::chain::tree::tx_root(txs);
        };
        match kind {
            Forge::Oversize => {
                while txs.iter().map(|t| t.len()).sum::<usize>() < soft3::chain::params::MAX_BLOCK_BYTES {
                    let k = txs.len() as u64;
                    txs.push(signed(&chain, Body::Transfer { from: self.donor.neuron, nonce: nonce + k, token: native(), amount: 1, to: [9; 32], fee: 300 }, &self.donor.key));
                }
                reroot(&mut header, &txs);
            }
            Forge::TxRoot => header.tx_root = self.rng.bytes32(),
            Forge::StateRoot => header.state_root = self.rng.bytes32(),
            Forge::CoinbaseAmount => {
                let mut cb = Tx::decode(&txs[0]).ok()?;
                if let Body::Coinbase { amount, .. } = &mut cb.body {
                    *amount += 1_000_000;
                }
                txs[0] = cb.encode();
                reroot(&mut header, &txs);
            }
            Forge::BadSigTx => {
                let i = txs[1].len() - 3;
                txs[1][i] ^= 1;
                reroot(&mut header, &txs);
            }
            Forge::DuplicateTx => {
                txs.push(txs[1].clone());
                reroot(&mut header, &txs);
            }
            Forge::TooManyTxs => {
                let base = txs[1].clone();
                while txs.len() <= soft3::chain::params::MAX_BLOCK_TXS {
                    let mut t = base.clone();
                    let k = txs.len();
                    t[10] ^= (k & 0xff) as u8;
                    t[11] ^= (k >> 8) as u8;
                    txs.push(t);
                }
                reroot(&mut header, &txs);
            }
            Forge::FutureTime => header.time = now + 3 * 3600,
            Forge::WrongBits => {
                let t = soft3::chain::net::codec::decode_bits(header.bits)?;
                header.bits = soft3::chain::net::codec::encode_bits(t.shl(1));
            }
        }
        let body = soft3::chain::tx::join_body(&txs);
        World::grind(&mut header);
        Some((header, body))
    }
}

impl Agent for Forger {
    fn name(&self) -> String {
        "forger".into()
    }

    fn act(&mut self, w: &mut World) -> Option<u64> {
        if self.next >= self.kinds.len() {
            return None;
        }
        if !w.nodes[self.node].online {
            return Some(MIN);
        }
        let kind = self.kinds[self.next];
        self.next += 1;
        if let Some((header, body)) = self.build(w, kind) {
            let id = header.id();
            self.forged.push((kind, id, header.height));
            w.nodes[self.node].forged.insert(id, (header, body));
            w.nodes[self.node].gossip(&Frame::HeaderGossip(header), None);
            w.flush(self.node);
        }
        Some(2 * MIN)
    }

    fn finish(&mut self, w: &mut World) {
        let honest = w.honest_online();
        for (kind, id, height) in &self.forged {
            let on_main = honest.iter().filter(|n| w.nodes[**n].chain().header_at(*height).map(|h| h.id()) == Some(*id)).count();
            w.checks.expect(&format!("forged block {kind:?}: never on an honest main chain"), on_main == 0, format!("height {height}; on {on_main}/{} honest main chains", honest.len()));
        }
        let banned_honest: Vec<String> = honest
            .iter()
            .flat_map(|n| w.nodes[*n].stats.bans.iter().filter(|(_, p)| honest.contains(p)).map(move |(t, p)| format!("{} banned node {} at {} s", n, p, t / 1000)))
            .collect();
        w.checks.expect("forged blocks: no honest relay banned", banned_honest.is_empty(), if banned_honest.is_empty() { "no honest peer banned".into() } else { banned_honest.join("; ") });
    }
}

// ── the economy's strategies ───────────────────────────────────────

/// One position an economy agent takes.
#[derive(Clone, Debug)]
pub struct Position {
    pub label: String,
    pub account: usize,
    pub p: Id,
    pub q: Id,
    pub valence: i8,
    pub amount: u64,
    /// epoch in which to lock (the tx goes out early in the epoch unless `late`)
    pub epoch: u64,
    /// lock in the last blocks before the boundary (the front-runner)
    pub late: bool,
    /// unlock everything at this epoch (once MIN_LOCK allows), then relock in the next
    pub recycle_at: Option<u64>,
    /// change the payout owner to this account index right after the boundary of `epoch`
    pub payout_to: Option<usize>,
    pub done: u8,
}

/// The economy driver: positions placed at chosen epochs from the run's
/// accounts, through one home node; the payout analysis runs over the
/// final main chain (`economy::analyze`).
pub struct Stakers {
    pub accounts: Vec<Account>,
    pub positions: Vec<Position>,
    pub node: usize,
    pub length: u64,
    pub min_lock: u64,
    pub log: Vec<String>,
}

impl Stakers {
    pub fn card(&self, p: &Position) -> Id {
        card_id(&self.accounts[p.account].neuron, &p.p, &p.q)
    }
}

impl Agent for Stakers {
    fn name(&self) -> String {
        "stakers".into()
    }

    fn act(&mut self, w: &mut World) -> Option<u64> {
        if !w.nodes[self.node].online {
            return Some(30 * SEC);
        }
        let chain = chain_id(w);
        let tip = w.nodes[self.node].tip().height;
        let next = tip + 1;
        let epoch_of_next = next.div_ceil(self.length);
        let blocks_left = epoch_of_next * self.length - tip; // blocks until (and including) the boundary
        let mut sends: Vec<Vec<u8>> = Vec::new();
        for i in 0..self.positions.len() {
            let pos = self.positions[i].clone();
            let a = &self.accounts[pos.account];
            let nonce_base = next_nonce(w, self.node, &a.neuron);
            let mut nonce = nonce_base;
            // stage 0: lock
            if pos.done == 0 && epoch_of_next == pos.epoch && (!pos.late || blocks_left <= 2) && (pos.late || blocks_left > 6) {
                sends.push(signed(&chain, Body::Cyberlink { from: a.neuron, nonce, token: native(), amount: pos.amount, p: pos.p, q: pos.q, valence: pos.valence, fee: 400 }, &a.key));
                nonce += 1;
                self.positions[i].done = 1;
                self.log.push(format!("{}: lock {} v{} at height ~{next}", pos.label, pos.amount, pos.valence));
            }
            // stage 1: payout owner change right after the lock's boundary
            if pos.done == 1 && pos.payout_to.is_some() && epoch_of_next == pos.epoch + 1 && blocks_left + 3 >= self.length {
                let to = self.accounts[pos.payout_to.unwrap()].neuron;
                sends.push(signed(&chain, Body::Payout { from: a.neuron, nonce, p: pos.p, q: pos.q, payout: to, fee: 400 }, &a.key));
                nonce += 1;
                self.positions[i].done = 2;
                self.log.push(format!("{}: payout owner → account {} at height ~{next}", pos.label, pos.payout_to.unwrap()));
            }
            // stage 1→3: recycle — unlock everything once allowed, then relock the same side
            if let Some(at) = pos.recycle_at {
                let card = self.card(&pos);
                let stake = w.nodes[self.node].chain().economy(|c| c.card_json(&card)).and_then(|v| v["stake"].as_u64()).unwrap_or(0);
                let unlock_at = w.nodes[self.node].chain().economy(|c| c.card_json(&card)).and_then(|v| v["unlock_at"].as_u64()).unwrap_or(u64::MAX);
                if pos.done == 1 && epoch_of_next >= at && next >= unlock_at && stake > 0 {
                    sends.push(signed(&chain, Body::Unlock { from: a.neuron, nonce, p: pos.p, q: pos.q, amount: stake, fee: 400 }, &a.key));
                    nonce += 1;
                    self.positions[i].done = 3;
                    self.positions[i].epoch = epoch_of_next + 1;
                    self.log.push(format!("{}: unlock {stake} at height ~{next}", pos.label));
                } else if pos.done == 3 && stake == 0 && epoch_of_next == self.positions[i].epoch && blocks_left > 6 {
                    sends.push(signed(&chain, Body::Cyberlink { from: a.neuron, nonce, token: native(), amount: pos.amount, p: pos.p, q: pos.q, valence: pos.valence, fee: 400 }, &a.key));
                    nonce += 1;
                    self.positions[i].done = 4;
                    self.log.push(format!("{}: relock {} at height ~{next}", pos.label, pos.amount));
                }
            }
            let _ = nonce;
            for tx in sends.drain(..) {
                if !submit(w, self.node, &tx) {
                    let why = w.nodes[self.node].submit_tx(&tx).err().map(|e| e.to_string()).unwrap_or_default();
                    self.log.push(format!("{}: refused: {why}", pos.label));
                    // retry the stage on the next action
                    self.positions[i].done = pos.done;
                    self.positions[i].epoch = pos.epoch;
                    break;
                }
            }
        }
        Some(10 * SEC)
    }
}
