//! The network + economy simulator of the cyber phase-1 chain.
//!
//! N in-process nodes run the real soft3 kernel (`chain::engine::Chain`
//! behind `chain::net::kernel::KernelChain`), connected by a simulated
//! network on a virtual clock; miners find blocks at simulated hashrate
//! shares and grind genuinely valid headers; agents (honest, adversarial,
//! economic) send real signed transactions; invariants are checked
//! continuously and every run ends in a verdict table. See README.md
//! § chain and audit/network-2026-10.md.

pub mod agents;
pub mod check;
pub mod economy;
pub mod genesis;
pub mod metrics;
pub mod net;
pub mod node;
pub mod report;
pub mod rng;
pub mod scenarios;
pub mod world;
pub mod launch;

/// The CLI (shared by `simulation chain …` and the `simulation-chain` binary).
pub fn cli(args: &[String]) -> i32 {
    let mut names: Vec<String> = Vec::new();
    let mut ctx = scenarios::Ctx { out: scenarios::default_out(), seed: 1, log: false, budget_s: 0 };
    let mut md: Option<std::path::PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = || {
            i += 1;
            args.get(i).cloned().unwrap_or_default()
        };
        match a {
            "--seed" => ctx.seed = val().parse().unwrap_or(1),
            "--out" => ctx.out = val().into(),
            "--md" => md = Some(val().into()),
            "--budget" => ctx.budget_s = val().parse().unwrap_or(0),
            "--log" => ctx.log = true,
            "--list" => {
                for s in ["gate", "nightly", "launch"] {
                    println!("{s}: {}", scenarios::suite(s).unwrap().join(" "));
                }
                return 0;
            }
            "-h" | "--help" => {
                println!("simulation chain <gate|nightly|launch|scenario…> [--seed N] [--out DIR] [--md FILE] [--budget S] [--log] [--list]");
                return 0;
            }
            s => names.push(s.to_string()),
        }
        i += 1;
    }
    if names.is_empty() {
        names.push("gate".into());
    }
    let mut list: Vec<String> = Vec::new();
    for n in &names {
        match scenarios::suite(n) {
            Some(s) => list.extend(s.iter().map(|x| x.to_string())),
            None => list.push(n.clone()),
        }
    }
    let _ = std::fs::create_dir_all(&ctx.out);
    let mut runs = Vec::new();
    for name in &list {
        eprintln!("▶ {name}");
        let r = scenarios::run(name, &ctx);
        let fails = r.rows.iter().filter(|x| x.status == check::Status::Fail).count();
        eprintln!("  {} — {} rows, {} failed, virtual {}, wall {:.1} s", name, r.rows.len(), fails, report::hms(r.virtual_s), r.wall_ms as f64 / 1000.0);
        for row in r.rows.iter().filter(|x| x.status == check::Status::Fail) {
            eprintln!("    FAIL {}: {}", row.invariant, row.evidence);
        }
        runs.push(r);
        // homes of finished scenarios are disposable
        let _ = std::fs::remove_dir_all(ctx.out.join("homes"));
    }
    let text = format!("## summary\n\n{}\n## verdicts\n\n{}", report::summary(&runs), report::table(&runs));
    println!("{text}");
    if let Some(p) = md {
        let _ = std::fs::write(p, &text);
    }
    if report::failed(&runs) { 1 } else { 0 }
}
