//! The launch suite: the real genesis files, real difficulty, the first
//! epochs (filled in below).

use std::time::Instant;

use crate::scenarios::{Ctx, Run};

pub fn run(_ctx: &Ctx, chain: &str, t0: Instant) -> Run {
    let mut c = crate::check::Checks::new(&format!("launch-{chain}"));
    c.info("launch", "not yet implemented");
    Run { name: format!("launch-{chain}"), rows: c.rows(), digest: String::new(), virtual_s: 0, wall_ms: t0.elapsed().as_millis() as u64, blocks: 0, notes: vec![] }
}
