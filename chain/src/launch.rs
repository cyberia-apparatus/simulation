//! The launch suite: the genesis day on the real files.
//!
//! The real pussy / bostrom genesis (`~/.cache/cyber/genesis/*.tade`), the
//! real `POW_LIMIT` (this process never relaxes it), the real genesis
//! target 2^232 and the spec's epoch clock (L 240, D 120). Miners join from
//! zero hashrate over the first half hour up to ≈ 1.1× the genesis design
//! point (559 kH/s, specs/chain.md §8), so every block is ground for real —
//! about 2^24 hashes each, on the Apple GPU with `--features metal`, else on
//! every CPU core. The mempool starts empty; the first stake comes from a
//! miner's matured coinbase, so epoch 1 settles real positions.

use std::sync::Arc;
use std::time::Instant;

use soft3::chain::economy::focus::Tru;
use soft3::chain::engine::OpenOptions;
use soft3::chain::mine::job::Job;
use soft3::chain::mine::target::target_of;
use soft3::chain::mine::Backend;
use soft3::chain::net::chain_view::ChainView;
use soft3::chain::net::codec::Header;
use soft3::chain::params::EpochClock;

use crate::agents::{Position, Stakers, Traffic};
use crate::genesis::Account;
use crate::metrics::Metrics;
use crate::net::LinkSpec;
use crate::node::Config;
use crate::rng::Rng;
use crate::scenarios::{home_of, intervals, Ctx, Run};
use crate::world::{Strategy, World, HOUR, MIN, SEC};

/// The conformance scalars of the genesis files (soft3 conformance/chain-genesis.toml).
const GENESIS_TOML: &str = include_str!("../../../soft3/conformance/chain-genesis.toml");

fn pinned(chain: &str, key: &str) -> Option<String> {
    let t: toml_lite::Table = toml_lite::parse(GENESIS_TOML);
    t.get(chain, key)
}

/// The grinding backend: Metal when built and present, else every CPU core.
pub fn backend() -> Box<dyn Backend> {
    if let Ok(b) = soft3::chain::mine::metal() {
        return b;
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).max(2) - 1;
    Box::new(soft3::chain::mine::cpu::Cpu::new(threads))
}

/// The least valid nonce from 0 (deterministic across backends).
pub fn grind_real(backend: &dyn Backend, header: &mut Header) -> u64 {
    let target = target_of(header.bits).expect("canonical bits");
    let job = Job::new(&header.encode(), target);
    let chunk = backend.dispatch().max(1 << 18);
    let mut base: u128 = 0;
    loop {
        if let Some(i) = backend.search(&job, base, chunk).expect("backend") {
            header.nonce = (base + i as u128).to_le_bytes();
            debug_assert!(header.valid_pow());
            return (base + i as u128) as u64 + 1;
        }
        base += chunk as u128;
    }
}

pub fn run(ctx: &Ctx, chain: &str, t0: Instant) -> Run {
    let name = format!("launch-{chain}");
    let mut checks = crate::check::Checks::new(&name);
    if soft3::chain::params::sim_pow::relaxed() {
        checks.fail("launch: real POW_LIMIT", "this process relaxed POW_LIMIT for a test genesis; run the launch suite on its own (`simulation chain launch`)");
        return Run { name, rows: checks.rows(), digest: String::new(), virtual_s: 0, wall_ms: 0, blocks: 0, notes: vec![] };
    }
    let genesis = match crate::genesis::launch(chain) {
        Ok(g) => g,
        Err(e) => {
            checks.fail("launch: genesis file present", &e);
            return Run { name, rows: checks.rows(), digest: String::new(), virtual_s: 0, wall_ms: 0, blocks: 0, notes: vec![] };
        }
    };
    let hex = soft3::chain::net::codec::hex;
    let particle = hex(&genesis.chain_id);
    let want = pinned(chain, "particle").unwrap_or_default();
    let backend: Arc<dyn Backend> = Arc::from(backend());
    let bname = backend.name();
    let cfg = Arc::new(Config {
        genesis: Arc::new(genesis.bytes.clone()),
        opts: OpenOptions { checkpoints: true, clock: None, engine: Some(Arc::new(Tru::new(4))), quadrature: 0, inline_settlement: false },
        clock: EpochClock::SPEC,
        supply0: genesis.s0(),
        subsidy: genesis.subsidy(),
        t0_unix: genesis.params().time,
    });
    let metrics = Metrics::to(Some(&ctx.out.join(format!("{name}-{}.jsonl", ctx.seed))));
    let mut w = World::new(&name, ctx.seed, cfg, metrics);
    w.log = ctx.log;
    w.deadline = ctx.budget_s;
    w.checks.expect("launch: chain id = the pinned genesis particle", particle == want, format!("{} (pinned {})", &particle[..16], &want[..want.len().min(16)]));
    let b = backend.clone();
    w.grinder = Some(Box::new(move |h: &mut Header| grind_real(b.as_ref(), h)));
    let home = home_of(ctx, &name);
    let n_nodes = if chain == "bostrom" { 3 } else { 4 };
    let ids: Vec<usize> = (0..n_nodes).map(|i| w.add_node(&format!("n{i}"), home.join(format!("n{i}")))).collect();
    let mut rng = Rng::new(ctx.seed ^ 0x1a);
    // the late joiner is the last node: it starts when the others are past the first settlement
    let early = &ids[..n_nodes - 1];
    w.net.mesh(&mut rng, &ids, 2, LinkSpec::WAN);
    let mut opened = Vec::new();
    for &i in early {
        let t = Instant::now();
        w.start_node(i);
        opened.push(t.elapsed().as_millis() as u64);
    }
    let late = ids[n_nodes - 1];
    let b0 = w.nodes[0].chain().header_at(0).map(|h| hex(&h.id())).unwrap_or_default();
    let root0 = w.nodes[0].chain().header_at(0).map(|h| hex(&h.state_root)).unwrap_or_default();
    let (want_b0, want_root) = (pinned(chain, "block0_id").unwrap_or_default(), pinned(chain, "state_root").unwrap_or_default());
    w.checks.expect("launch: block 0 and state root = the pinned values", b0 == want_b0 && root0 == want_root, format!("block 0 {} root {}; opened {} nodes in {:?} ms; RSS {} MB", &b0[..16], &root0[..16], early.len(), opened, crate::world::rss_mb()));
    // miners join from zero: 100 kH/s at once, +200 at 10 min, +300 at 30 min (rates in mH/s)
    let accounts: Vec<Account> = (0..8u64).map(|i| Account::new(7_000_000 + i)).collect();
    let kh = 1_000_000u64; // 1 kH/s in mH/s
    let m0 = w.add_miner_to("cpu", ids[0], 100 * kh, Strategy::Honest, accounts[0].neuron);
    let _ = m0;
    let (n1, n2) = (ids[1], ids[2 % n_nodes]);
    let a1 = accounts[1].neuron;
    let a2 = accounts[2].neuron;
    w.at(10 * MIN, move |w| {
        w.add_miner_to("gpu-a", n1, 200 * kh, Strategy::Honest, a1);
    });
    w.at(30 * MIN, move |w| {
        w.add_miner_to("gpu-b", n2, 300 * kh, Strategy::Honest, a2);
    });
    // the first stake: the CPU miner locks a quarter of its matured coinbase on genesis edges in epoch 1
    let gl = &genesis.file.links;
    let edge = |k: usize| (gl[k % gl.len()].p, gl[k % gl.len()].q);
    let mut positions = Vec::new();
    for (k, v) in [(0usize, 1i8), (7, 1), (13, -1)] {
        let (p, q) = edge(k * 997);
        positions.push(Position { label: format!("miner-stake{k}"), account: 0, p, q, valence: v, amount: 0, epoch: 1, late: false, recycle_at: None, payout_to: None, done: 0 });
    }
    // amounts are set when the coinbase matures (a quarter of the balance, split)
    let stake_amount = 2 * genesis.subsidy();
    for p in positions.iter_mut() {
        p.amount = stake_amount;
    }
    w.add_agent(55 * MIN, Box::new(Stakers { accounts: accounts.clone(), positions: positions.clone(), node: ids[0], length: 240, min_lock: soft3::chain::params::MIN_LOCK, log: vec![] }));
    w.add_agent(
        70 * MIN,
        Box::new(Traffic { accounts: accounts[..3].to_vec(), nodes: early.to_vec(), every_ms: 20 * SEC, fee_bonus: 0, rng: Rng::new(ctx.seed ^ 3), sent: Default::default(), refused: 0, label: "traffic from matured coinbases".into(), stop_at: 12 * HOUR, min_share: 90 }),
    );
    w.stall_ms = 40 * MIN;
    let target_height = EpochClock::SPEC.settle(2) + 10;
    let mut late_started = None;
    let mut first_block_ms = None;
    while w.now < 12 * HOUR {
        let next = w.now + 10 * MIN;
        w.run_until(next);
        let h = w.nodes[0].tip().height;
        if first_block_ms.is_none() && h >= 1 {
            first_block_ms = w.main_chain(0).first().and_then(|b| b.2).map(|f| f.0);
        }
        if late_started.is_none() && h >= EpochClock::SPEC.settle(1) + 30 {
            let t = Instant::now();
            w.start_node(late);
            late_started = Some((w.now, t.elapsed().as_millis() as u64, h));
        }
        if w.log {
            eprintln!("  [{name}] virtual {} min, height {h}, hashes {} M, wall {} s", w.now / MIN, w.hash_budget / 1_000_000, t0.elapsed().as_secs());
        }
        if h >= target_height && late_started.is_some_and(|(_, _, _)| w.nodes[late].tip().height + 2 >= h) {
            break;
        }
        if w.checks.failed() && w.checks.rows().iter().any(|r| r.invariant == "wall-clock budget") {
            break;
        }
    }
    let h = w.nodes[0].tip().height;
    w.checks.expect("launch: the chain reaches settle(2) + 10", h >= target_height, format!("height {h} at virtual {} min ({} M hashes ground on {bname})", w.now / MIN, w.hash_budget / 1_000_000));
    let first = first_block_ms.unwrap_or(0);
    w.checks.info("launch: first block at genesis difficulty (2^232)", &format!("found at virtual {} s with 100 kH/s mining (expected ≈ 168 s)", first / 1000));
    let (mean, p99, max, n) = intervals(&w, 0, 60 * MIN, w.now);
    w.checks.expect("launch: mean interval 30 s ± 25 % once miners joined", n >= 50 && (22_500..=37_500).contains(&mean), format!("{n} blocks after 60 min, mean {:.1} s, p99 {} s, max {} s", mean as f64 / 1000.0, p99 / 1000, max / 1000));
    if let Some((at, ms, height)) = late_started {
        let lt = w.nodes[late].tip().height;
        w.checks.expect("launch: late joiner syncs from genesis through two settlements", lt + 2 >= h, format!("started at virtual {} min (network at {height}), opened in {ms} ms, now at {lt} of {h}", at / MIN));
    }
    // settlement compute per node against SETTLE_BUDGET
    let mut timings = Vec::new();
    for i in &ids {
        if let Some(v) = w.nodes[*i].chain().economy(|c| Some(c.economy_status())) {
            for t in v["timings"].as_array().cloned().unwrap_or_default() {
                timings.push(format!("{}:e{} {} ms ({} axons)", w.nodes[*i].name, t["epoch"], t["millis"], t["axons"]));
            }
        }
    }
    let max_ms = ids
        .iter()
        .filter_map(|i| w.nodes[*i].chain().economy(|c| Some(c.economy_status())))
        .flat_map(|v| v["timings"].as_array().cloned().unwrap_or_default())
        .filter_map(|t| t["millis"].as_u64())
        .max()
        .unwrap_or(0);
    w.checks.expect("launch: settlement compute ≤ SETTLE_BUDGET (900 s)", max_ms <= soft3::chain::params::SETTLE_BUDGET_SECS * 1000, format!("max {max_ms} ms; {}", timings.join(", ")));
    // the record's syntropy fits its i64 (economy-2026-10-17 finding 3)
    let recs = crate::economy::records(&w, ids[0]);
    let saturated: Vec<String> = recs.iter().filter(|r| r.syntropy == i64::MAX || r.syntropy == i64::MIN || r.delta == i64::MAX || r.delta == i64::MIN).map(|r| format!("epoch {} syntropy {}", r.epoch, r.syntropy)).collect();
    if chain == "bostrom" {
        // economy-2026-10-17 finding 3: J(φ*_0) on bostrom exceeds i64 at 2^60
        w.checks.expected_red.push("launch: record syntropy fits i64 (not saturated)".into());
    }
    w.checks.expect("launch: record syntropy fits i64 (not saturated)", saturated.is_empty(), if saturated.is_empty() { format!("{} records, syntropy {:?}", recs.len(), recs.iter().map(|r| r.syntropy).collect::<Vec<_>>()) } else { saturated.join("; ") });
    crate::economy::analyze(&mut w, &accounts, &positions, 240);
    w.quiesce(5 * MIN);
    w.converged();
    w.agents_finish();
    let conservation: Vec<String> = w.nodes.iter().flat_map(|n| n.stats.conservation_failures.clone()).collect();
    w.checks.expect("conservation every block (Σ native = S0 + h·SUBSIDY)", conservation.is_empty(), if conservation.is_empty() { format!("{} applications", w.nodes.iter().map(|n| n.stats.applied).sum::<u64>()) } else { conservation.join("; ") });
    let apply_max = w.nodes.iter().map(|n| n.stats.apply_us_max).max().unwrap_or(0);
    w.checks.expect("resource: per-block apply time ≤ 2 s (settlement blocks included)", apply_max <= 2_000_000, format!("max {} ms", apply_max / 1000));
    w.checks.info("resource: peak RSS", &format!("{} MB for {} nodes", crate::world::rss_mb(), n_nodes));
    w.checks.pass_if_quiet("liveness: no stall", "no honest node waited longer than 40 min");
    w.checks.pass_if_quiet("safety: agree below finality", "honest nodes agreed below depth 120 at every sample");
    w.metrics.flush();
    let digest = w.digest();
    Run { name, rows: w.checks.rows(), digest, virtual_s: w.now / 1000, wall_ms: t0.elapsed().as_millis() as u64, blocks: h, notes: w.notes.clone() }
}

/// A minimal reader for the flat `[section] key = "value"` file above.
mod toml_lite {
    pub struct Table(Vec<(String, String, String)>);

    pub fn parse(s: &str) -> Table {
        let mut out = Vec::new();
        let mut section = String::new();
        for line in s.lines() {
            let l = line.trim();
            if l.starts_with('[') && l.ends_with(']') {
                section = l[1..l.len() - 1].to_string();
            } else if let Some((k, v)) = l.split_once('=') {
                let v = v.trim();
                if let Some(rest) = v.strip_prefix('"') {
                    if let Some(end) = rest.find('"') {
                        out.push((section.clone(), k.trim().to_string(), rest[..end].to_string()));
                    }
                } else {
                    out.push((section.clone(), k.trim().to_string(), v.split('#').next().unwrap_or("").trim().to_string()));
                }
            }
        }
        Table(out)
    }

    impl Table {
        pub fn get(&self, section: &str, key: &str) -> Option<String> {
            self.0.iter().find(|(s, k, _)| s == section && k == key).map(|x| x.2.clone())
        }
    }
}
