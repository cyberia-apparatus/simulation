//! The verdict table (markdown) and the run summary.

use crate::check::Status;
use crate::scenarios::Run;

pub fn table(runs: &[Run]) -> String {
    let mut out = String::from("| scenario | invariant | verdict | evidence |\n|---|---|---|---|\n");
    for r in runs {
        for row in &r.rows {
            let ev = row.evidence.replace('|', "\\|").replace('\n', " ");
            out.push_str(&format!("| {} | {} | {} | {} |\n", r.name, row.invariant, row.status.word(), ev));
        }
    }
    out
}

pub fn summary(runs: &[Run]) -> String {
    let mut out = String::from("| scenario | virtual | wall | blocks | pass | fail | expected red | info | digest |\n|---|---|---|---|---|---|---|---|---|\n");
    for r in runs {
        let count = |s: Status| r.rows.iter().filter(|x| x.status == s).count();
        out.push_str(&format!(
            "| {} | {} | {:.1} s | {} | {} | {} | {} | {} | `{}` |\n",
            r.name,
            hms(r.virtual_s),
            r.wall_ms as f64 / 1000.0,
            r.blocks,
            count(Status::Pass),
            count(Status::Fail),
            count(Status::ExpectedRed) + count(Status::UnexpectedGreen),
            count(Status::Info),
            &r.digest[..r.digest.len().min(12)]
        ));
    }
    out
}

pub fn hms(s: u64) -> String {
    format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
}

pub fn failed(runs: &[Run]) -> bool {
    runs.iter().any(|r| r.rows.iter().any(|x| x.status == Status::Fail))
}
