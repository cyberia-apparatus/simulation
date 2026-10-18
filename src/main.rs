//! closed-loop emergence simulator.
//!
//! a sparse graph grows by a hidden rule (preferential attachment +
//! triadic closure — the physics the model cannot see). a fraction
//! `--g` of link events instead follows the compiled model's top
//! suggestion, closing the loop of crystal/emergence.md: the graph
//! starts encoding model-accessible regularities. the compiled
//! embedding (spectrum of D(A + 0.5 A^2) D, the shipped tru operator)
//! is recomputed on a cadence. metrics stream as JSONL.

mod graph;
mod kernels;
mod metrics;
mod model;
mod rng;
mod spectrum;

use graph::Graph;
use kernels::Backend;
use metrics::Probes;
use model::Model;
use rng::Rng;
use spectrum::Spectrum;

struct Config {
    nodes: usize,
    g: f64,
    q: f64,
    d: usize,
    compile_every: u64,
    probe: usize,
    seen_probe: usize,
    seed: u64,
    iters: usize,
    kernels: String,
}

fn parse_args() -> Config {
    let mut c = Config {
        nodes: 1_000_000,
        g: 0.0,
        q: 0.5,
        d: 64,
        compile_every: 200_000,
        probe: 500,
        seen_probe: 500,
        seed: 7,
        iters: 25,
        kernels: "scalar".into(),
    };
    let args: Vec<String> = std::env::args().collect();
    // `simulation emergence --flags…` and the historical `simulation --flags…`
    let mut i = if args.get(1).map(String::as_str) == Some("emergence") { 2 } else { 1 };
    while i < args.len() {
        let take = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i)
                .cloned()
                .unwrap_or_else(|| die("flag needs a value"))
        };
        match args[i].as_str() {
            "--nodes" => c.nodes = take(&mut i).parse().unwrap_or_else(|_| die("bad --nodes")),
            "--g" => c.g = take(&mut i).parse().unwrap_or_else(|_| die("bad --g")),
            "--q" => c.q = take(&mut i).parse().unwrap_or_else(|_| die("bad --q")),
            "--d" => c.d = take(&mut i).parse().unwrap_or_else(|_| die("bad --d")),
            "--compile-every" => {
                c.compile_every = take(&mut i)
                    .parse()
                    .unwrap_or_else(|_| die("bad --compile-every"))
            }
            "--probe" => c.probe = take(&mut i).parse().unwrap_or_else(|_| die("bad --probe")),
            "--seen-probe" => {
                c.seen_probe = take(&mut i)
                    .parse()
                    .unwrap_or_else(|_| die("bad --seen-probe"))
            }
            "--seed" => c.seed = take(&mut i).parse().unwrap_or_else(|_| die("bad --seed")),
            "--iters" => c.iters = take(&mut i).parse().unwrap_or_else(|_| die("bad --iters")),
            "--kernels" => c.kernels = take(&mut i),
            other => die(&format!("unknown flag {other}")),
        }
        i += 1;
    }
    c
}

fn die(msg: &str) -> ! {
    eprintln!("simulation: {msg}");
    std::process::exit(2)
}

/// pick a source node ∝ total degree (rejection sampling against max).
fn pick_source(g: &Graph, rng: &mut Rng) -> u32 {
    loop {
        let v = rng.below(g.n) as u32;
        let d = g.degree[v as usize];
        if d > 0 && rng.f64() < d as f64 / g.max_degree.max(1) as f64 {
            return v;
        }
    }
}

/// hidden-rule target for source s: triadic closure with prob q
/// (neighbor-of-neighbor), else preferential attachment. excludes
/// self-links and duplicates.
fn hidden_target(g: &Graph, s: u32, q: f64, rng: &mut Rng) -> u32 {
    for _ in 0..32 {
        let t = if rng.f64() < q {
            let nbrs = g.out_neighbors(s);
            if nbrs.is_empty() {
                continue;
            }
            let u = nbrs[rng.below(nbrs.len())];
            let un = g.out_neighbors(u);
            if un.is_empty() {
                continue;
            }
            un[rng.below(un.len())]
        } else {
            let v = rng.below(g.n) as u32;
            let d = g.degree[v as usize];
            if d == 0 || rng.f64() >= d as f64 / g.max_degree.max(1) as f64 {
                continue;
            }
            v
        };
        if t != s && !g.is_linked(s, t) {
            return t;
        }
    }
    // fall back to any unlinked node
    for _ in 0..64 {
        let t = rng.below(g.n) as u32;
        if t != s && !g.is_linked(s, t) && g.degree[t as usize] > 0 {
            return t;
        }
    }
    u32::MAX
}

fn main() {
    // `simulation chain …`: the network + economy simulator of the phase-1 chain
    #[cfg(feature = "chain")]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.get(1).map(String::as_str) == Some("chain") {
            std::process::exit(simulation_chain::cli(&args[2..]));
        }
    }
    let cfg = parse_args();
    let mut rng = Rng::new(cfg.seed);
    let mut g = Graph::new(cfg.nodes + 16);

    // seed clique so every node has a path in
    let m0 = 8u32;
    for _ in 0..m0 {
        g.add_node();
    }
    for a in 0..m0 {
        for b in (a + 1)..m0 {
            g.link(a, b);
            g.link(b, a);
        }
    }

    // probe schedule: the last `probe` link events of the run are
    // pre-committed: their (s, t) is chosen by the hidden rule NOW but
    // added to the graph only at their event index. everything the
    // model ranks stays unseen by construction.
    let target_edges = cfg.nodes as u64 * 3; // avg out-degree ~3 at target size
    let probe_start = target_edges - cfg.probe as u64;
    let mut probes = Probes {
        unseen: Vec::with_capacity(cfg.probe),
        seen: Vec::new(),
    };

    let spec = Spectrum::new(cfg.iters);
    let backend = Backend::parse(&cfg.kernels);
    let kern = backend.kernels();
    let mut probes_generated = false;
    #[allow(unused_assignments)]
    let mut emb_store: Option<spectrum::Embedding> = None;
    let mut model: Option<Model> = None;
    let mut next_compile: u64 = cfg.compile_every;
    let mut event: u64 = g.edges.len() as u64;
    let t_start = std::time::Instant::now();

    println!("{{\"event\": {event}, \"n\": {}, \"edges\": {}, \"phase\": \"start\", \"g\": {}, \"q\": {}}}",
             g.n, g.edges.len(), cfg.g, cfg.q);

    while g.n < cfg.nodes || event < target_edges {
        // pick source; half the time grow the graph with a new node as
        // the target — keeps the graph connected and growing. probe
        // events never create fresh nodes: their recorded target must
        // be exactly the edge the graph will get.
        // at the start of the probe window: pre-generate ALL reserved
        // probes from the current state (hidden rule), then take a
        // forecast reading — the model ranks future links it has never
        // seen. each probe event later links exactly its reserved edge.
        if !probes_generated && event >= probe_start {
            for i in 0..cfg.probe {
                let ps = pick_source(&g, &mut rng);
                let pt = hidden_target(&g, ps, cfg.q, &mut rng);
                if pt != u32::MAX {
                    probes.unseen.push((probe_start + i as u64, ps, pt));
                }
            }
            probes_generated = true;
            next_compile = event; // forecast reading this very tick
        }
        let s = pick_source(&g, &mut rng);
        let is_probe = event >= probe_start;
        let fresh = !is_probe && g.n < cfg.nodes && rng.f64() < 0.5;
        let t = if fresh {
            u32::MAX // fresh events link to a new node below; no target needed
        } else if event >= probe_start {
            let pidx = (event - probe_start) as usize;
            if pidx < probes.unseen.len() {
                probes.unseen[pidx].2
            } else {
                event += 1;
                continue;
            }
        } else {
            let use_model = cfg.g > 0.0 && model.is_some() && rng.f64() < cfg.g;
            if use_model {
                let m = model.as_mut().unwrap();
                let cand = m.suggest(s as usize, g.out_neighbors(s));
                if cand != usize::MAX {
                    cand as u32
                } else {
                    hidden_target(&g, s, cfg.q, &mut rng)
                }
            } else {
                let tgt = hidden_target(&g, s, cfg.q, &mut rng);
                if tgt == u32::MAX {
                    event += 1;
                    continue;
                }
                tgt
            }
        };
        if fresh {
            g.add_node();
            g.link(s, (g.n - 1) as u32);
        } else {
            if t == u32::MAX {
                event += 1;
                continue;
            }
            g.link(s, t);
        }
        event += 1;

        if event >= next_compile {
            let tc = std::time::Instant::now();
            emb_store = Some(spec.compile(&g, cfg.d, &mut rng, kern.as_ref()));
            let compile_s = tc.elapsed().as_secs_f64();
            let mut m = Model::new(emb_store.as_ref().unwrap());
            let (unseen, npend) = metrics::eval_unseen(&mut m, &g, &probes, event);
            let seen = if probes.seen.is_empty() {
                // sample seen probes once, from train edges present now
                for _ in 0..cfg.seen_probe {
                    let idx = rng.below(g.edges.len());
                    let (s2, t2) = g.edges[idx];
                    probes.seen.push((s2, t2));
                }
                metrics::eval_seen(&mut m, &g, &probes)
            } else {
                metrics::eval_seen(&mut m, &g, &probes)
            };
            let s1 = emb_store
                .as_ref()
                .unwrap()
                .sigma
                .first()
                .copied()
                .unwrap_or(0.0);
            let sk = emb_store
                .as_ref()
                .unwrap()
                .sigma
                .last()
                .copied()
                .unwrap_or(0.0);
            println!(
                "{{\"event\": {event}, \"n\": {}, \"edges\": {}, \"unseen\": {unseen:.4}, \"unseen_n\": {npend}, \"seen\": {seen:.4}, \"sigma1\": {s1:.4}, \"sigma_ratio\": {:.2}, \"compile_s\": {compile_s:.2}, \"wall_s\": {:.1}}}",
                g.n,
                g.edges.len(),
                s1 / sk.max(1e-12),
                t_start.elapsed().as_secs_f64()
            );
            model = Some(m);
            next_compile += cfg.compile_every;
        }
    }

    // final metrics
    if let Some(mut m) = model.take() {
        let (unseen, npend) = metrics::eval_unseen(&mut m, &g, &probes, u64::MAX);
        let seen = metrics::eval_seen(&mut m, &g, &probes);
        println!(
            "{{\"event\": {event}, \"n\": {}, \"edges\": {}, \"unseen\": {unseen:.4}, \"unseen_n\": {npend}, \"seen\": {seen:.4}, \"phase\": \"final\", \"wall_s\": {:.1}}}",
            g.n,
            g.edges.len(),
            t_start.elapsed().as_secs_f64()
        );
    }
}
