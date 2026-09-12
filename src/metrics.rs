//! metrics: unseen-MRR on reserved future links, seen-MRR on train
//! edges. probes are generated before the run and enter the graph only
//! at their scheduled event, so compilation never sees them.

use crate::graph::Graph;
use crate::model::Model;

pub struct Probes {
    /// (event_index, source, actual_target) — reserved future links;
    /// only probes with event_index > current are "pending"
    pub unseen: Vec<(u64, u32, u32)>,
    /// (source, target) sampled train edges — memorization control
    pub seen: Vec<(u32, u32)>,
}

pub fn eval_unseen(model: &mut Model, g: &Graph, probes: &Probes, event: u64) -> (f64, usize) {
    let mut acc = 0.0;
    let mut n = 0;
    for &(at, s, t) in &probes.unseen {
        if at <= event {
            continue; // already happened — no longer an unseen probe
        }
        let linked = g.out_neighbors(s);
        let rank = model.rank_of(s as usize, t as usize, linked);
        acc += 1.0 / rank as f64;
        n += 1;
    }
    (if n == 0 { 0.0 } else { acc / n as f64 }, n)
}

pub fn eval_seen(model: &mut Model, g: &Graph, probes: &Probes) -> f64 {
    let mut acc = 0.0;
    for &(s, t) in &probes.seen {
        let linked = g.out_neighbors(s);
        let rank = model.rank_of(s as usize, t as usize, linked);
        acc += 1.0 / rank as f64;
    }
    acc / probes.seen.len() as f64
}
