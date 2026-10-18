//! `simulation-chain <suite | scenario …> [--seed N] [--out DIR] [--md FILE] [--budget S] [--log]`
//!
//! Suites: `gate`, `nightly`, `launch` (see `scenarios.rs`). Writes JSONL
//! metrics per scenario under `--out` (default /tmp/simulation-chain), the
//! verdict table to `--md` (and stdout); exits 1 on any failed invariant
//! (expected reds do not fail).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(simulation_chain::cli(&args));
}
