//! Scenarios and suites. `gate` (≤ 10 min on one M4 Max; the release
//! train and every soft3 merge), `nightly` (hours, larger N, longer
//! horizons, the stronger adversaries), `launch` (the real pussy and
//! bostrom genesis files, real difficulty, the first epochs).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use soft3::chain::economy::focus::{Focus, FocusEngine, FocusGraph, Tru};
use soft3::chain::engine::OpenOptions;
use soft3::chain::net::chain_view::ChainView;
use soft3::chain::params::{EpochClock, FINALITY_DEPTH};

use crate::agents::{Flood, Forge, Forger, Hostile, Junk, Position, Stakers, Traffic};
use crate::check::{Row, Status};
use crate::genesis::{self, particle, Account, TestGenesis, TestSpec};
use crate::metrics::Metrics;
use crate::net::LinkSpec;
use crate::node::{Config, Role};
use crate::rng::Rng;
use crate::world::{Strategy, World, HOUR, MIN, SEC};

pub struct Ctx {
    pub out: PathBuf,
    pub seed: u64,
    pub log: bool,
    /// wall-clock budget per scenario, seconds (0 = none)
    pub budget_s: u64,
}

pub struct Run {
    pub name: String,
    pub rows: Vec<Row>,
    pub digest: String,
    pub virtual_s: u64,
    pub wall_ms: u64,
    pub blocks: u64,
    pub notes: Vec<String>,
}

pub const GATE: &[&str] = &[
    "steady", "determinism", "partition-heal", "crash-restart", "late-join", "clock-skew", "asert-step", "selfish", "reorg-60", "reorg-140", "eclipse",
    "flood", "hostile", "economy", "settle-slow", "settle-panic",
];

pub const NIGHTLY: &[&str] = &[
    "steady-large", "partition-long", "asert-100x", "selfish-40", "reorg-140-checkpoints", "eclipse-long", "forged-header-storm", "flood-long", "economy-long",
    "settle-slow-one",
];

pub const LAUNCH: &[&str] = &["launch-pussy", "launch-bostrom"];

pub fn suite(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "gate" => Some(GATE),
        "nightly" => Some(NIGHTLY),
        "launch" => Some(LAUNCH),
        _ => None,
    }
}

// ── the test network ───────────────────────────────────────────────

pub struct TestNet {
    pub w: World,
    pub tg: Arc<TestGenesis>,
    pub base_rate: u64,
    pub honest: Vec<usize>,
}

pub struct NetSpec {
    pub nodes: usize,
    pub degree: usize,
    pub link: LinkSpec,
    pub accounts: usize,
    pub legacy: usize,
    pub clock: Option<EpochClock>,
    /// honest hashrate shares in ppm of the base rate, one miner per entry on nodes 0, 1, …
    pub miners: Vec<u64>,
    pub engine: Option<Arc<dyn FocusEngine>>,
}

impl Default for NetSpec {
    fn default() -> Self {
        NetSpec {
            nodes: 8,
            degree: 3,
            link: LinkSpec::WAN,
            accounts: 64,
            legacy: 4,
            clock: Some(EpochClock { length: 40, delay: 20, min_lock: 60 }),
            miners: vec![400_000, 150_000, 150_000, 150_000, 150_000],
            engine: None,
        }
    }
}

pub fn home_of(ctx: &Ctx, scenario: &str) -> PathBuf {
    let d = ctx.out.join("homes").join(format!("{scenario}-{}", ctx.seed));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The genesis links: a dominant edge, a small cluster, a tail.
fn genesis_links() -> Vec<(usize, u64, u64)> {
    let mut v = vec![(0, 1, 2), (1, 1, 2), (2, 2, 3), (3, 3, 1), (4, 1, 4), (5, 4, 5), (6, 5, 6), (7, 6, 1)];
    for i in 0..40u64 {
        v.push(((i % 8) as usize, 10 + i, 10 + (i * 7 + 3) % 40));
    }
    v
}

pub fn testnet(ctx: &Ctx, scenario: &str, spec: NetSpec) -> TestNet {
    soft3::chain::params::sim_pow::relax();
    let tg = Arc::new(genesis::test(&TestSpec { accounts: spec.accounts, funds: 1_000_000_000_000, legacy: spec.legacy, legacy_funds: 5_000_000, links: genesis_links() }));
    let engine: Arc<dyn FocusEngine> = spec.engine.unwrap_or_else(|| Arc::new(Tru::new(1)));
    let cfg = Arc::new(Config {
        genesis: Arc::new(tg.genesis.bytes.clone()),
        opts: OpenOptions { checkpoints: true, clock: spec.clock, engine: Some(engine), quadrature: 0, inline_settlement: false },
        clock: spec.clock.unwrap_or(EpochClock::SPEC),
        supply0: tg.genesis.s0(),
        subsidy: tg.genesis.subsidy(),
        t0_unix: tg.genesis.params().time,
    });
    let metrics = Metrics::to(Some(&ctx.out.join(format!("{scenario}-{}.jsonl", ctx.seed))));
    let mut w = World::new(scenario, ctx.seed, cfg, metrics);
    w.log = ctx.log;
    w.deadline = ctx.budget_s;
    let home = home_of(ctx, scenario);
    let ids: Vec<usize> = (0..spec.nodes).map(|i| w.add_node(&format!("n{i}"), home.join(format!("n{i}")))).collect();
    let mut rng = Rng::new(ctx.seed ^ 0x70b0);
    w.net.mesh(&mut rng, &ids, spec.degree, spec.link);
    for &i in &ids {
        w.start_node(i);
    }
    let target = soft3::chain::net::codec::decode_bits(tg.genesis.params().bits).expect("bits");
    let work = soft3::chain::net::codec::work(target).0[0];
    let base_rate = work * 100 / 3; // mH/s for a 30 s block
    for (k, share) in spec.miners.iter().enumerate() {
        let node = ids[k % ids.len()];
        w.add_miner(&format!("m{k}"), node, base_rate * share / 1_000_000, Strategy::Honest);
    }
    let chain_id = tg.genesis.chain_id;
    w.signer = Some((ids[0], tg.checkpoint_key.clone(), chain_id));
    TestNet { w, tg, base_rate, honest: ids }
}

fn traffic(t: &TestNet, every_ms: u64, stop_at: u64, label: &str, seed: u64) -> Box<Traffic> {
    Box::new(Traffic {
        accounts: t.tg.accounts[..t.tg.accounts.len().min(32)].to_vec(),
        nodes: t.honest.clone(),
        every_ms,
        fee_bonus: 0,
        rng: Rng::new(seed),
        sent: Default::default(),
        refused: 0,
        label: label.into(),
        stop_at,
        min_share: 90,
    })
}

/// Block interval statistics on `node`'s main chain between virtual times.
pub fn intervals(w: &World, node: usize, from: u64, to: u64) -> (u64, u64, u64, u64) {
    let main = w.main_chain(node);
    let times: Vec<u64> = main.iter().filter_map(|(_, _, f)| f.map(|x| x.0)).filter(|t| *t >= from && *t <= to).collect();
    if times.len() < 2 {
        return (0, 0, 0, times.len() as u64);
    }
    let gaps: Vec<u64> = times.windows(2).map(|p| p[1].saturating_sub(p[0])).collect();
    let mean = gaps.iter().sum::<u64>() / gaps.len() as u64;
    let max = *gaps.iter().max().unwrap_or(&0);
    let mut sorted = gaps.clone();
    sorted.sort_unstable();
    let p99 = sorted[(sorted.len() - 1) * 99 / 100];
    (mean, p99, max, times.len() as u64)
}

/// Common end of a run: quiesce, converge, the continuous invariants.
fn finish(w: &mut World, quiesce: u64) {
    w.quiesce(quiesce);
    w.converged();
    w.agents_finish();
    let mut conservation = Vec::new();
    let mut panics = Vec::new();
    let mut apply_max = 0u64;
    let mut apply_all: Vec<u64> = Vec::new();
    let mut tpl_max = 0;
    let mut upkeep_max = 0u64;
    let mut honest_bans = Vec::new();
    let honest: Vec<usize> = w.nodes.iter().filter(|n| n.role == Role::Honest).map(|n| n.ix).collect();
    let attackers: Vec<usize> = w.nodes.iter().filter(|n| n.role != Role::Honest).map(|n| n.ix).collect();
    let hostile: Vec<usize> = attackers.iter().chain(w.hostile_ix.iter()).chain(w.expect_split.iter()).copied().collect();
    let mut charges: std::collections::BTreeMap<String, u64> = Default::default();
    for n in w.nodes.iter().filter(|n| n.role == Role::Honest) {
        for (k, v) in &n.stats.charges {
            *charges.entry(k.clone()).or_default() += v;
        }
    }
    w.metrics.emit(serde_json::json!({"kind": "charges", "by_honest_nodes": charges}));
    for n in &w.nodes {
        conservation.extend(n.stats.conservation_failures.iter().cloned());
        panics.extend(n.stats.panics.iter().cloned());
        apply_max = apply_max.max(n.stats.apply_us_max);
        apply_all.extend(n.stats.apply_us.iter().copied());
        tpl_max = tpl_max.max(n.stats.template_us_max);
        upkeep_max = upkeep_max.max(n.stats.upkeep_us_max);
        if hostile.contains(&n.ix) || n.role != Role::Honest {
            continue;
        }
        for (k, (t, p)) in n.stats.bans.iter().enumerate() {
            if honest.contains(p) && !hostile.contains(p) {
                let why = n.stats.ban_reasons.get(k).cloned().unwrap_or_default();
                honest_bans.push(format!("{} banned {} at {} s ({why})", n.name, w.nodes[*p].name, t / 1000));
            }
        }
    }
    apply_all.sort_unstable();
    let p99 = apply_all.get(apply_all.len().saturating_sub(1) * 99 / 100).copied().unwrap_or(0);
    let applied: u64 = w.nodes.iter().map(|n| n.stats.applied).sum();
    w.checks.expect("conservation every block (Σ native = S0 + h·SUBSIDY)", conservation.is_empty(), if conservation.is_empty() { format!("{applied} block applications on {} nodes", w.nodes.len()) } else { conservation[..conservation.len().min(3)].join("; ") });
    w.checks.expect("no node panicked", panics.is_empty(), if panics.is_empty() { "none".into() } else { panics.join("; ") });
    w.checks.expect("resource: per-block apply time ≤ 2 s", apply_max <= 2_000_000, format!("p99 {} ms, max {} ms over {} applications; template max {} ms", p99 / 1000, apply_max / 1000, apply_all.len(), tpl_max / 1000));
    if !w.allow_honest_bans {
        w.checks.expect("no honest peer banned", honest_bans.is_empty(), if honest_bans.is_empty() { "none".into() } else { honest_bans[..honest_bans.len().min(4)].join("; ") });
    }
    w.checks.expect("resource: mempool upkeep per tip move ≤ 2 s", upkeep_max <= 2_000_000, format!("max {} ms (remove mined + re-admit the pool)", upkeep_max / 1000));
    let rss = crate::world::rss_mb();
    w.checks.info("resource: peak RSS", &format!("{rss} MB (process)"));
    let mempool_max = w.nodes.iter().map(|n| n.stats.mempool_max).max().unwrap_or(0);
    w.checks.expect("resource: mempool ≤ capacity", mempool_max <= soft3::chain::net::mempool::CAPACITY_TXS, format!("max {mempool_max} txs"));
    w.checks.pass_if_quiet("liveness: no stall", "no honest node waited longer than the stall bound");
    w.checks.pass_if_quiet("safety: agree below finality", "honest nodes agreed below depth 120 at every sample");
    w.checks.pass_if_quiet("safety: reorg depth ≤ 119", "no reorganization deeper than MAX_REORG");
    w.metrics.flush();
}

fn interval_check(w: &mut World, node: usize, from: u64, to: u64, lo: u64, hi: u64, label: &str) {
    let (mean, p99, max, n) = intervals(w, node, from, to);
    w.checks.expect(label, n >= 10 && mean >= lo && mean <= hi, format!("{n} blocks, mean {:.1} s, p99 {} s, max {} s (bound {}–{} s)", mean as f64 / 1000.0, p99 / 1000, max / 1000, lo / 1000, hi / 1000));
}

fn stale_rate(w: &World, node: usize) -> (u64, u64) {
    let main: std::collections::BTreeSet<[u8; 32]> = w.main_chain(node).iter().map(|(_, h, _)| h.id()).collect();
    let found = w.found.len() as u64;
    let stale = w.found.keys().filter(|id| !main.contains(*id)).count() as u64;
    (stale, found)
}

fn done(w: World, name: &str, t0: Instant) -> Run {
    let mut w = w;
    let digest = w.digest();
    let obs = w.honest_online().first().copied().unwrap_or(0);
    let blocks = if w.nodes[obs].online { w.nodes[obs].tip().height } else { 0 };
    let mut notes = std::mem::take(&mut w.notes);
    notes.truncate(20);
    Run { name: name.into(), rows: w.checks.rows(), digest, virtual_s: w.now / 1000, wall_ms: t0.elapsed().as_millis() as u64, blocks, notes }
}

// ── scenarios ──────────────────────────────────────────────────────

pub fn run(name: &str, ctx: &Ctx) -> Run {
    let t0 = Instant::now();
    match name {
        "steady" => steady(ctx, name, 8, 2 * HOUR + 30 * MIN, t0),
        "steady-large" => steady(ctx, name, 24, 8 * HOUR, t0),
        "determinism" => determinism(ctx, t0),
        "partition-heal" => partition(ctx, name, 25 * MIN, t0),
        "partition-long" => partition(ctx, name, 2 * HOUR, t0),
        "crash-restart" => crash_restart(ctx, name, t0),
        "late-join" => late_join(ctx, name, t0),
        "clock-skew" => clock_skew(ctx, name, t0),
        "asert-step" => asert_step(ctx, name, 10, t0),
        "asert-100x" => asert_step(ctx, name, 100, t0),
        "selfish" => selfish(ctx, name, 330_000, 3 * HOUR, t0),
        "selfish-40" => selfish(ctx, name, 400_000, 8 * HOUR, t0),
        "reorg-60" => reorg(ctx, name, 60, false, t0),
        "reorg-140" => reorg(ctx, name, 140, false, t0),
        "reorg-140-checkpoints" => reorg(ctx, name, 140, true, t0),
        "eclipse" => eclipse(ctx, name, 40 * MIN, 30 * MIN, t0),
        "eclipse-long" => eclipse(ctx, name, 40 * MIN, 2 * HOUR + 30 * MIN, t0),
        "flood" => flood(ctx, name, 10 * MIN, t0),
        "flood-long" => flood(ctx, name, 2 * HOUR, t0),
        "hostile" => hostile(ctx, name, 9, t0),
        "forged-header-storm" => hostile(ctx, name, 130, t0),
        "economy" => economy(ctx, name, 8, t0),
        "economy-long" => economy(ctx, name, 24, t0),
        "settle-slow" => settle_slow(ctx, name, false, t0),
        "settle-slow-one" => settle_slow(ctx, name, true, t0),
        "settle-panic" => settle_panic(ctx, name, t0),
        "launch-pussy" => crate::launch::run(ctx, "pussy", t0),
        "launch-bostrom" => crate::launch::run(ctx, "bostrom", t0),
        other => {
            let mut c = crate::check::Checks::new(other);
            c.fail("scenario exists", &format!("unknown scenario {other}"));
            Run { name: other.into(), rows: c.rows(), digest: String::new(), virtual_s: 0, wall_ms: 0, blocks: 0, notes: vec![] }
        }
    }
}

fn steady(ctx: &Ctx, name: &str, nodes: usize, dur: u64, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes, degree: if nodes > 10 { 4 } else { 3 }, ..Default::default() });
    let tr = traffic(&t, 4 * SEC, dur, "honest traffic", ctx.seed ^ 1);
    t.w.add_agent(10 * SEC, tr);
    t.w.run_until(dur);
    let obs = t.honest[1];
    interval_check(&mut t.w, obs, 10 * MIN, dur, 24 * SEC, 36 * SEC, "liveness: mean block interval 30 s ± 20 %");
    let (stale, found) = stale_rate(&t.w, obs);
    t.w.checks.expect("stale blocks ≤ 5 %", stale * 20 <= found, format!("{stale} of {found} found blocks off the main chain"));
    let cps = t.w.nodes[obs].checkpoints.len();
    t.w.checks.expect("checkpoints reach every node", t.w.honest_online().iter().all(|n| t.w.nodes[*n].checkpoints.len() == cps) && cps > 0, format!("{cps} checkpoints held by node {obs}"));
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn determinism(ctx: &Ctx, t0: Instant) -> Run {
    let one = |tag: &str, seed: u64| {
        let c = Ctx { out: ctx.out.join(tag), seed, log: false, budget_s: ctx.budget_s };
        let mut t = testnet(&c, "determinism", NetSpec { nodes: 5, ..Default::default() });
        let tr = traffic(&t, 5 * SEC, 50 * MIN, "traffic", seed ^ 1);
        t.w.add_agent(10 * SEC, tr);
        t.w.at(20 * MIN, |w| w.partition(&[vec![0, 1], vec![2, 3, 4]]));
        t.w.at(30 * MIN, |w| w.heal());
        t.w.run_until(50 * MIN);
        t.w.quiesce(3 * MIN);
        (t.w.digest(), t.w.nodes[0].tip().height)
    };
    let (a, ha) = one("a", ctx.seed);
    let (b, hb) = one("b", ctx.seed);
    let (c, _) = one("c", ctx.seed + 1);
    let mut checks = crate::check::Checks::new("determinism");
    checks.expect("determinism: same seed ⇒ bit-identical run", a == b && ha == hb, format!("digest {} vs {} (height {ha} vs {hb})", &a[..16], &b[..16]));
    checks.expect("determinism: another seed ⇒ another run", a != c, format!("seed+1 digest {}", &c[..16]));
    Run { name: "determinism".into(), rows: checks.rows(), digest: a, virtual_s: 53 * 60, wall_ms: t0.elapsed().as_millis() as u64, blocks: ha, notes: vec![] }
}

fn partition(ctx: &Ctx, name: &str, len: u64, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes: 10, degree: 3, ..Default::default() });
    let mut tr = traffic(&t, 6 * SEC, len + 2 * HOUR, "traffic", ctx.seed ^ 1);
    if len > HOUR {
        // the minority's pending transactions die with its stores at the operator resync
        tr.min_share = 60;
    }
    t.w.add_agent(10 * SEC, tr);
    // majority: nodes 0, 2, 4–7 (miners m0 40 %, m2, m4 → 70 %); minority: 1, 3, 8, 9 (m1, m3 → 30 %)
    let a = vec![0, 2, 4, 5, 6, 7];
    let b = vec![1, 3, 8, 9];
    let start = 30 * MIN;
    let long = len > HOUR;
    let (aa, bb) = (a.clone(), b.clone());
    t.w.at(start, move |w| {
        w.partition(&[aa, bb.clone()]);
        if long {
            // two chains that will not rejoin (chain.md §5.2): the minority is measured apart
            w.expect_split = bb;
        }
    });
    let bb = b.clone();
    t.w.at(start + len, move |w| {
        w.heal();
        if len > HOUR {
            // chain.md §5.2: a partition longer than an hour does not rejoin by itself
            w.expect_split = bb;
        }
    });
    let mut end = start + len + 30 * MIN;
    if long {
        // the minority operator resyncs from a checkpoint: wipe and restart
        let bb = b.clone();
        t.w.at(start + len + 20 * MIN, move |w| {
            let fork_left: Vec<String> = bb.iter().map(|n| w.nodes[*n].short()).collect();
            w.notes.push(format!("after heal, minority nodes {}", fork_left.join(" ")));
            for n in &bb {
                w.crash_node(*n);
                let _ = std::fs::remove_dir_all(&w.nodes[*n].home);
            }
        });
        let bb = b.clone();
        t.w.at(start + len + 21 * MIN, move |w| {
            for n in &bb {
                w.start_node(*n);
            }
            w.expect_split.clear();
        });
        end += 30 * MIN;
    }
    t.w.stall_ms = 30 * MIN;
    t.w.run_until(end);
    // the minority's reorg on heal
    let depths: Vec<u64> = b.iter().flat_map(|n| t.w.nodes[*n].stats.reorgs.iter().map(|r| r.1)).collect();
    let refusals: u64 = b.iter().map(|n| t.w.nodes[*n].stats.deep_refusals).sum();
    if long {
        t.w.checks.info("partition > 1 h: minority refuses the deep reorg (chain.md §5.2)", &format!("{refusals} deep-fork refusals on minority nodes before the operator resync"));
    } else {
        t.w.checks.expect("partition < 1 h heals by reorg", depths.iter().all(|d| *d <= 119) && !depths.is_empty(), format!("minority reorg depths {depths:?}"));
    }
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn crash_restart(ctx: &Ctx, name: &str, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes: 8, ..Default::default() });
    let tr = traffic(&t, 5 * SEC, 2 * HOUR, "traffic", ctx.seed ^ 1);
    t.w.add_agent(10 * SEC, tr);
    // a miner node, a relay node, and a node crashed across an epoch boundary + settlement
    t.w.at(20 * MIN, |w| w.crash_node(1));
    t.w.at(32 * MIN, |w| w.start_node(1));
    t.w.at(40 * MIN, |w| w.crash_node(6));
    t.w.at(41 * MIN, |w| w.start_node(6));
    t.w.at(50 * MIN, |w| {
        w.crash_node(0);
        w.crash_node(7);
    });
    t.w.at(80 * MIN, |w| {
        w.start_node(0);
        w.start_node(7);
    });
    t.w.run_until(2 * HOUR);
    let replays: Vec<String> = t.w.notes.iter().filter(|n| n.contains("opened")).cloned().collect();
    t.w.checks.info("crash: restart replays from disk", &format!("{} slow opens; nodes 0, 1, 6, 7 restarted", replays.len()));
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn late_join(ctx: &Ctx, name: &str, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes: 6, ..Default::default() });
    let tr = traffic(&t, 5 * SEC, 3 * HOUR, "traffic", ctx.seed ^ 1);
    t.w.add_agent(10 * SEC, tr);
    let home = home_of(ctx, &format!("{name}-late"));
    let late: Vec<usize> = (0..3).map(|i| t.w.add_node(&format!("late{i}"), home.join(format!("late{i}")))).collect();
    for (k, &l) in late.iter().enumerate() {
        let at = (60 + 40 * k as u64) * MIN;
        let spec = if k == 2 { LinkSpec::SLOW } else { LinkSpec::WAN };
        t.w.at(at, move |w| {
            w.link(l, k % 6, spec);
            w.link(l, (k + 3) % 6, spec);
            w.start_node(l);
        });
    }
    t.w.run_until(3 * HOUR);
    let hs: Vec<String> = late.iter().map(|l| t.w.nodes[*l].short()).collect();
    t.w.checks.info("late joiners: initial block download", &format!("{}", hs.join(", ")));
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn clock_skew(ctx: &Ctx, name: &str, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes: 8, ..Default::default() });
    let skews = [0i64, 100, -100, 60, -60, 200, -150, 30];
    for (i, s) in skews.iter().enumerate() {
        t.w.nodes[i].skew_s = *s;
    }
    let tr = traffic(&t, 6 * SEC, 2 * HOUR, "traffic", ctx.seed ^ 1);
    t.w.add_agent(10 * SEC, tr);
    t.w.run_until(2 * HOUR);
    let held: u64 = t.w.nodes.iter().map(|n| n.stats.held).sum();
    t.w.checks.info("clock skew ±200 s: future-drift holds", &format!("{held} headers or blocks held for the 120 s drift rule, re-evaluated on the tick"));
    interval_check(&mut t.w, 1, 10 * MIN, 2 * HOUR, 22 * SEC, 38 * SEC, "liveness under skew: mean interval 30 s ± 25 %");
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn asert_step(ctx: &Ctx, name: &str, factor: u64, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes: 6, miners: vec![1_000_000], ..Default::default() });
    let base = t.base_rate;
    // ×f for 90 min, back to base; then ÷f from base for 150 min, back to base
    let (up, down, end) = (60 * MIN, 4 * HOUR, 9 * HOUR);
    t.w.at(up, move |w| w.set_rate(0, base * factor));
    t.w.at(up + 90 * MIN, move |w| w.set_rate(0, base));
    t.w.at(down, move |w| w.set_rate(0, base / factor));
    t.w.at(down + 2 * HOUR + 30 * MIN, move |w| w.set_rate(0, base));
    t.w.stall_ms = 6 * HOUR;
    t.w.run_until(end);
    let obs = 1;
    let recovery = |w: &World, from: u64, to: u64| -> u64 {
        // first 20-block window after `from` whose mean is within 30 s ± 20 %
        let main = w.main_chain(obs);
        let times: Vec<u64> = main.iter().filter_map(|(_, _, f)| f.map(|x| x.0)).filter(|t| *t >= from && *t <= to).collect();
        for k in 0..times.len().saturating_sub(20) {
            let mean = (times[k + 20] - times[k]) / 20;
            if (24_000..=36_000).contains(&mean) {
                return (times[k] - from) / 1000;
            }
        }
        u64::MAX
    };
    let r_up = recovery(&t.w, up, up + 90 * MIN);
    let r_down = recovery(&t.w, down, down + 2 * HOUR + 30 * MIN);
    // analytic: ASERT halves the target per 600 s of schedule error; a ×f step
    // needs log2(f) half-lives of *excess* blocks to absorb
    t.w.checks.expect(&format!("ASERT ×{factor} spike: interval back to 30 s ± 20 %"), r_up < 3 * 3600, format!("{} s after the step", r_up));
    t.w.checks.expect(&format!("ASERT ÷{factor} collapse: interval back to 30 s ± 20 %"), r_down < 3 * 3600, format!("{} s after the step", r_down));
    // the first block after a collapse is mined at the old target (ASERT reads the parent only): ≈ f × 30 s
    let first_after = t.w.main_chain(obs).iter().filter_map(|(_, _, f)| f.map(|x| x.0)).find(|x| *x >= down).map(|x| (x - down) / 1000);
    let (_, _, max_gap, _) = intervals(&t.w, obs, down.saturating_sub(10 * MIN), down + 2 * HOUR);
    t.w.checks.info(&format!("ASERT ÷{factor}: first block after the collapse, longest gap"), &format!("first block {} s after the step (expected ≈ {} s), longest gap {} s", first_after.unwrap_or(0), factor * 30, max_gap / 1000));
    interval_check(&mut t.w, obs, down + 2 * HOUR + 30 * MIN + 30 * MIN, end, 22 * SEC, 38 * SEC, "liveness: steady again after the steps");
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn selfish(ctx: &Ctx, name: &str, alpha_ppm: u64, dur: u64, t0: Instant) -> Run {
    let honest_left = 1_000_000 - alpha_ppm;
    let mut t = testnet(ctx, name, NetSpec { nodes: 9, miners: vec![honest_left * 2 / 5, honest_left / 5, honest_left / 5, honest_left / 5], ..Default::default() });
    let atk = 8;
    t.w.nodes[atk].role = Role::Withholder;
    let rate = t.base_rate * alpha_ppm / 1_000_000;
    t.w.add_miner("selfish", atk, rate, Strategy::Selfish { race: false });
    // the attacker is well connected (γ > 0): link it to every honest node
    for n in 0..8 {
        t.w.link(atk, n, LinkSpec { latency_ms: 10, jitter_ms: 5, loss_ppm: 0, bandwidth: 10_000_000 });
    }
    t.w.expect_split = vec![atk];
    t.w.allow_honest_bans = false;
    t.w.run_until(dur);
    let obs = 0;
    let main = t.w.main_chain(obs);
    let on_main: u64 = main.iter().filter(|(_, _, f)| f.is_some_and(|x| t.w.miners[x.1].name == "selfish")).count() as u64;
    let total = main.iter().filter(|(_, _, f)| f.is_some()).count() as u64;
    let share = on_main * 1_000_000 / total.max(1);
    let deepest = t.w.nodes.iter().flat_map(|n| n.stats.reorgs.iter().map(|r| r.1)).max().unwrap_or(0);
    t.w.checks.info(&format!("selfish mining at α = {} %: revenue share", alpha_ppm / 10_000), &format!("{:.1} % of {} main-chain blocks (hashrate {:.1} %); deepest honest reorg {deepest}", share as f64 / 10_000.0, total, alpha_ppm as f64 / 10_000.0));
    interval_check(&mut t.w, obs, 10 * MIN, dur, 22 * SEC, 40 * SEC, "liveness under selfish mining");
    t.w.checks.expect("selfish: honest reorgs stay shallow", deepest < 20, format!("deepest {deepest}"));
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn reorg(ctx: &Ctx, name: &str, depth: u64, checkpoints: bool, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes: 8, ..Default::default() });
    let atk = 7;
    if !checkpoints {
        t.w.signer = None;
    }
    let rate = t.base_rate * 3 / 2; // 60 % of the total once it starts
    let start = 40 * MIN;
    t.w.at(start, move |w| {
        w.nodes[atk].role = Role::Withholder;
        w.nodes[atk].deaf = true;
        w.expect_split = vec![atk];
        w.add_miner("attacker", atk, rate, Strategy::Private { depth, fork: None, released: false });
    });
    let home = home_of(ctx, &format!("{name}-late"));
    let late = t.w.add_node("late", home.join("late"));
    let end = start + (depth * 30 * SEC) * 2 + 60 * MIN;
    t.w.at(end - 40 * MIN, move |w| {
        w.link(late, 0, LinkSpec::WAN);
        w.link(late, atk, LinkSpec::WAN);
        w.start_node(late);
        if depth > 119 {
            // which chain it follows is the question, not a safety violation of the honest set
            w.expect_split = vec![atk, late];
        }
    });
    t.w.stall_ms = 40 * MIN;
    t.w.run_until(end);
    let honest: Vec<usize> = (0..7).collect();
    let deepest = honest.iter().flat_map(|n| t.w.nodes[*n].stats.reorgs.iter().map(|r| r.1)).max().unwrap_or(0);
    let refused: u64 = honest.iter().map(|n| t.w.nodes[*n].stats.deep_refusals).sum();
    let atk_tip = t.w.nodes[atk].tip();
    let on_attacker = honest.iter().filter(|n| t.w.nodes[**n].chain().header(&atk_tip.id).is_some() && t.w.nodes[**n].tip().id == atk_tip.id).count();
    if depth <= 119 {
        t.w.checks.expect(&format!("reorg {depth} < 120: honest nodes reorganize to the heavier chain"), deepest >= depth.min(40) && deepest <= 119, format!("deepest honest reorg {deepest}; {on_attacker}/7 on the attacker's tip"));
    } else {
        t.w.checks.expect(&format!("reorg {depth} ≥ 120: refused by every honest node (chain.md §5.2)"), deepest <= 119 && refused > 0, format!("deepest {deepest}; {refused} deep-fork refusals; {on_attacker}/7 on the attacker's tip"));
        // the late joiner sees both chains: which does it follow?
        let late_tip = t.w.nodes[late].tip();
        let follows_attacker = t.w.nodes[atk].chain().height_of(&late_tip.id).is_some() && t.w.nodes[0].chain().height_of(&late_tip.id).is_none();
        let ev = format!("late joiner at {} follows the {} chain; checkpoints {}", late_tip.height, if follows_attacker { "attacker's" } else { "honest" }, if checkpoints { "on" } else { "off" });
        if checkpoints {
            t.w.checks.expect("late joiner rejects a deep rewrite (checkpoints on)", !follows_attacker, ev);
        } else {
            t.w.checks.info("late joiner after a refused deep rewrite (checkpoints off)", &ev);
        }
        t.w.expect_split = vec![atk, late];
    }
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn eclipse(ctx: &Ctx, name: &str, start: u64, len: u64, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes: 8, ..Default::default() });
    let victim = 6;
    let atk = 7; // a synced node that turns: isolated with the victim, 5 % hashrate
    let rate = t.base_rate / 20;
    t.w.at(start, move |w| {
        let vp = w.net.all_links_of(victim);
        let ap = w.net.all_links_of(atk);
        for p in vp {
            w.net.set_up(victim, p, false);
            w.nodes[victim].disconnected(p);
            w.nodes[p].disconnected(victim);
        }
        for p in ap {
            w.net.set_up(atk, p, false);
            w.nodes[atk].disconnected(p);
            w.nodes[p].disconnected(atk);
        }
        w.link(victim, atk, LinkSpec::WAN);
        w.nodes[atk].role = Role::Withholder;
        w.add_miner("eclipser", atk, rate, Strategy::Honest);
        w.expect_split = vec![victim, atk];
    });
    let long = len > HOUR;
    t.w.at(start + len, move |w| {
        w.unlink(victim, atk);
        for p in w.net.all_links_of(victim) {
            w.net.set_up(victim, p, true);
            if w.nodes[p].online {
                w.nodes[victim].connected(p);
                w.nodes[p].connected(victim);
            }
        }
        w.nodes[victim].sync_pass(None);
        w.flush(victim);
        // a branch ≥ 120 deep cannot be undone (chain.md §5.2): the victim stays apart
        w.expect_split = if long { vec![atk, victim] } else { vec![atk] };
    });
    let end = start + len + 40 * MIN;
    t.w.stall_ms = 2 * HOUR;
    t.w.run_until(end);
    let v = t.w.nodes[victim].tip();
    let h0 = t.w.nodes[0].tip();
    let recovered = v.id == h0.id;
    let atk_blocks = t.w.miners.iter().find(|m| m.name == "eclipser").map(|m| m.found).unwrap_or(0);
    let ev = format!("eclipse {} min by a 5 % miner: attacker chain grew {atk_blocks} blocks; victim at {} ({}), honest at {}", len / MIN, v.height, if recovered { "recovered" } else { "stuck on the attacker's branch" }, h0.height);
    if atk_blocks <= 119 {
        t.w.checks.expect("eclipse: victim recovers when the fake branch is < 120 deep", recovered, ev);
    } else {
        // chain.md §5.2: a reorg beyond 119 is never taken; the operator resyncs.
        // A finding for the owner (the eclipser needs almost no hashrate), run as expected red.
        t.w.checks.expected_red.push("eclipse > 1 h: the victim rejoins without an operator".into());
        t.w.checks.add("eclipse > 1 h: the victim rejoins without an operator", if recovered { Status::Pass } else { Status::Fail }, ev);
        if !recovered {
            t.w.expect_split = vec![atk, victim];
        }
    }
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn flood(ctx: &Ctx, name: &str, len: u64, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes: 5, accounts: 600, ..Default::default() });
    let mut honest_traffic = traffic(&t, 5 * SEC, 30 * MIN + len, "honest txs during the flood (fee +100)", ctx.seed ^ 1);
    honest_traffic.fee_bonus = 100;
    honest_traffic.accounts = t.tg.accounts[..16].to_vec();
    t.w.add_agent(10 * SEC, honest_traffic);
    let flood = Box::new(Flood { accounts: t.tg.accounts[16..].to_vec(), node: 0, per_round: 200, every_ms: 2 * SEC, until: 20 * MIN + len, rng: Rng::new(ctx.seed ^ 2), admitted: 0, refused: 0, below_floor_admitted: 0 });
    t.w.add_agent(20 * MIN, flood);
    t.w.run_until(40 * MIN + len);
    let full: Vec<u64> = t.w.main_chain(1).iter().filter_map(|(h, _, _)| t.w.nodes[1].chain().block_at(*h).map(|(_, b)| b.len() as u64)).collect();
    let max_body = full.iter().max().copied().unwrap_or(0);
    t.w.checks.info("flood: block fullness", &format!("largest body {} KB of {} KB; {} blocks", max_body / 1024, soft3::chain::params::MAX_BLOCK_BYTES / 1024, full.len()));
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

fn hostile(ctx: &Ctx, name: &str, forged: usize, t0: Instant) -> Run {
    let mut t = testnet(ctx, name, NetSpec { nodes: 6, legacy: 4, ..Default::default() });
    let tr = traffic(&t, 5 * SEC, 2 * HOUR, "traffic", ctx.seed ^ 1);
    t.w.add_agent(10 * SEC, tr);
    // the legitimate owner of legacy account 0 claims it
    let l0 = t.tg.legacy[0].clone();
    let chain = t.tg.genesis.chain_id;
    t.w.at(5 * MIN, move |w| {
        let tx = crate::agents::signed(&chain, soft3::chain::tx::Body::Claim { account_id: l0.account_id, nonce: 0, fee: 300 }, &l0.key);
        crate::agents::submit(w, 0, &tx);
    });
    let home = home_of(ctx, &format!("{name}-hostile"));
    let mut hostile_ix = Vec::new();
    for (k, kind) in Junk::ALL.iter().enumerate() {
        let ix = t.w.add_node(&format!("x{k}"), home.join(format!("x{k}")));
        hostile_ix.push(ix);
        for n in 0..6 {
            if (n + k) % 2 == 0 {
                t.w.link(ix, n, LinkSpec::WAN);
            }
        }
        let legacy = if *kind == Junk::DoubleClaim { Some(t.tg.legacy[0].clone()) } else { Some(t.tg.legacy[1].clone()) };
        let donor = t.tg.accounts[40 + k].clone();
        t.w.at(1, move |w| {
            w.start_node(ix);
            w.nodes[ix].role = Role::Silent;
        });
        t.w.add_agent(15 * MIN + k as u64 * 20 * SEC, Box::new(Hostile { node: ix, kind: *kind, sent: 0, max: 12, rng: Rng::new(ctx.seed ^ (100 + k as u64)), donor, legacy }));
    }
    let fx = t.w.add_node("forger", home.join("forger"));
    hostile_ix.push(fx);
    for n in 0..6 {
        t.w.link(fx, n, LinkSpec::WAN);
    }
    t.w.at(1, move |w| {
        w.start_node(fx);
        w.nodes[fx].role = Role::Silent;
    });
    let kinds: Vec<Forge> = (0..forged).map(|i| Forge::ALL[i % Forge::ALL.len()]).collect();
    let storm = forged > Forge::ALL.len();
    t.w.add_agent(20 * MIN, Box::new(Forger { node: fx, kinds, next: 0, donor: t.tg.accounts[60].clone(), forged: vec![], rng: Rng::new(ctx.seed ^ 7) }));
    t.w.hostile_ix = hostile_ix.clone();
    t.w.expect_split = hostile_ix;
    let end = if storm { 20 * MIN + forged as u64 * 2 * MIN + 20 * MIN } else { 2 * HOUR };
    t.w.run_until(end);
    // the legacy owner was credited once
    let neuron = soft3::chain::sigs::neuron_of(&crate::genesis::pubkey(&t.tg.legacy[0].key));
    let bal = t.w.nodes[0].chain().balance(&neuron, &t.tg.native);
    t.w.checks.expect("legacy claim credited exactly once", bal == t.tg.legacy[0].amount - 300, format!("claimant holds {bal} of {} (fee 300); double claims, wrong-key claims refused", t.tg.legacy[0].amount));
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

/// The economy strategies on one short-epoch network.
fn economy(ctx: &Ctx, name: &str, epochs: u64, t0: Instant) -> Run {
    let clock = EpochClock { length: 40, delay: 20, min_lock: 60 };
    // node 0 runs tru on 1 thread, node 1 on 4: the records must still agree
    let mut t = testnet(ctx, name, NetSpec { nodes: 6, accounts: 96, clock: Some(clock), ..Default::default() });
    let tg = t.tg.clone();
    let accounts = tg.accounts.clone();
    let big = 5_000_000_000u64;
    let mut pos: Vec<Position> = Vec::new();
    fn mk(label: &str, account: usize, p: u64, q: u64, valence: i8, amount: u64, epoch: u64) -> Position {
        Position { label: label.into(), account, p: particle(p), q: particle(q), valence, amount, epoch, late: false, recycle_at: None, payout_to: None, done: 0 }
    }
    // honest diverse stakers on genesis and new edges
    let mut rng = Rng::new(ctx.seed ^ 0xec0);
    for i in 0..16 {
        let (p, q) = if i % 2 == 0 { (rng.range(10, 49), rng.range(10, 49)) } else { (rng.range(1, 6), rng.range(10, 49)) };
        let q = if p == q { q + 1 } else { q };
        let v = [1i8, 1, 0, -1][rng.below(4) as usize];
        pos.push(mk(&format!("honest{i}"), 10 + i, p, q, v, big / 4 + rng.below(big), 1 + rng.below(3)));
    }
    // epochs 1–3 carry every strategy; epoch 5 is left quiet for the recycler's relock
    // concentration: every epoch more stake on the dominant edge
    for e in 1..=3 {
        pos.push(mk(&format!("concentrate{e}"), 30 + e as usize, 1, 2, 1, big, e));
    }
    // recycler: lock, unlock at MIN_LOCK, relock the same side
    pos.push(Position { recycle_at: Some(4), ..mk("recycle", 40, 2, 3, 1, big, 2) });
    // hedger: +1 and −1 of equal size on one axon from two keys
    pos.push(mk("hedge+", 41, 3, 1, 1, big, 2));
    pos.push(mk("hedge-", 42, 3, 1, -1, big, 2));
    // Sybil: one key vs the same stake over 8 keys, same axon side, same epoch
    pos.push(mk("sybil-one", 43, 4, 5, 1, big, 3));
    for k in 0..8 {
        pos.push(mk(&format!("sybil-split{k}"), 44 + k, 4, 5, 1, big / 8, 3));
    }
    // a self-linking ring on fresh particles
    for k in 0..4u64 {
        pos.push(mk(&format!("ring{k}"), 52 + k as usize, 900 + k, 900 + (k + 1) % 4, 1, big, 2));
    }
    // a lock and its last-block copy on the same axon side
    pos.push(mk("original", 56, 5, 6, 1, big, 3));
    pos.push(Position { late: true, ..mk("frontrun", 57, 5, 6, 1, big, 3) });
    // −1 against the dominant edge
    pos.push(mk("grief", 58, 1, 2, -1, big, 3));
    // payout owner changed right after the lock's boundary
    pos.push(Position { payout_to: Some(60), ..mk("payout", 59, 6, 1, 1, big, 2) });
    let positions = pos.clone();
    let stakers = Box::new(Stakers { accounts: accounts.clone(), positions: pos, node: 2, length: clock.length, min_lock: clock.min_lock, log: vec![] });
    t.w.add_agent(30 * SEC, stakers);
    let tr = traffic(&t, 8 * SEC, (epochs + 2) * 40 * 30 * SEC, "traffic", ctx.seed ^ 1);
    t.w.add_agent(10 * SEC, tr);
    // known open findings, reproduced on purpose (economy-2026-10-17 §findings 1 and 4)
    t.w.checks.expected_red = vec!["economy §9: recycler earns ≤ the mark's decay".into(), "economy §9: hedge (+1/−1 equal) earns nothing".into()];
    let end = (epochs * clock.length + clock.delay + 10) * 30 * SEC;
    t.w.run_until(end);
    finish(&mut t.w, 5 * MIN);
    crate::economy::analyze(&mut t.w, &accounts, &positions, clock.length);
    t.w.metrics.flush();
    done(t.w, name, t0)
}

fn settle_slow(ctx: &Ctx, name: &str, one: bool, t0: Instant) -> Run {
    let clock = EpochClock { length: 40, delay: 20, min_lock: 60 };
    let mut t = testnet(ctx, name, NetSpec { nodes: 6, clock: Some(clock), ..Default::default() });
    // D = 20 blocks ≈ 10 min; the settlement takes 25 min of compute
    let slow = 25 * MIN;
    for n in 0..6 {
        if !one || n == 0 {
            t.w.nodes[n].settle_cpu_ms = slow;
        }
    }
    t.w.stall_ms = slow + 20 * MIN;
    let tr = traffic(&t, 6 * SEC, 4 * HOUR, "traffic", ctx.seed ^ 1);
    t.w.add_agent(10 * SEC, tr);
    t.w.run_until(4 * HOUR);
    let waits: u64 = t.w.nodes.iter().map(|n| n.stats.settle_waits).sum();
    let max_gap = t.w.nodes.iter().filter(|n| n.role == Role::Honest).map(|n| n.stats.max_tip_gap_ms).max().unwrap_or(0);
    let h = t.w.nodes[1].tip().height;
    t.w.checks.expect(
        "slow settlement: the chain waits at boundary + D, then resumes",
        h >= 3 * clock.length + clock.delay,
        format!("settlement compute {} min vs D = {} blocks; {waits} blocks deferred; longest tip gap {} min; height {h}", slow / MIN, clock.delay, max_gap / MIN),
    );
    finish(&mut t.w, 5 * MIN);
    done(t.w, name, t0)
}

/// A φ* engine that panics: what a node does when the settlement worker dies.
struct PanicEngine;

impl FocusEngine for PanicEngine {
    fn id(&self) -> &'static str {
        "panic"
    }
    fn focus(&self, _g: &FocusGraph) -> Focus {
        panic!("φ* engine failure (simulated)")
    }
    fn gradient(&self, _g: &FocusGraph) -> Vec<i128> {
        panic!("gradient engine failure (simulated)")
    }
}

fn settle_panic(ctx: &Ctx, name: &str, t0: Instant) -> Run {
    let clock = EpochClock { length: 40, delay: 20, min_lock: 60 };
    let mut t = testnet(ctx, name, NetSpec { nodes: 4, clock: Some(clock), engine: Some(Arc::new(PanicEngine)), ..Default::default() });
    t.w.stall_ms = 10 * HOUR;
    t.w.run_until(2 * HOUR);
    let dead = t.w.nodes.iter().filter(|n| !n.online).count();
    let h = t.w.nodes.iter().map(|n| n.stats.applied).max().unwrap_or(0);
    // restart one: does replay crash again?
    let crashed: Vec<usize> = t.w.nodes.iter().filter(|n| !n.online).map(|n| n.ix).collect();
    if let Some(&c) = crashed.first() {
        t.w.start_node(c);
    }
    let reopened = crashed.first().map(|c| t.w.nodes[*c].online).unwrap_or(false);
    let rows_before = t.w.checks.rows();
    let mut c = crate::check::Checks::new(name);
    c.info(
        "settlement engine panics: node behaviour",
        &format!(
            "{dead}/4 nodes crashed at settle(1) = height {} (highest applied {h}); a restart {} — a deterministic engine failure halts the chain (chain-economy §2.5: never guess)",
            clock.settle(1),
            if reopened { "reopens below settle(1) and crashes again on the next block" } else { "fails to reopen" }
        ),
    );
    let _ = rows_before;
    Run { name: name.into(), rows: c.rows(), digest: String::new(), virtual_s: t.w.now / 1000, wall_ms: t0.elapsed().as_millis() as u64, blocks: h, notes: vec![] }
}

/// Where a scenario's homes and metrics go.
pub fn default_out() -> PathBuf {
    Path::new("/tmp/simulation-chain").to_path_buf()
}

pub fn accounts_of(t: &TestNet) -> Vec<Account> {
    t.tg.accounts.clone()
}

pub const FINALITY: u64 = FINALITY_DEPTH;
