//! Verdicts: one row per (scenario, invariant) with a status and the
//! evidence. A failure is loud: the run's exit code is non-zero unless every
//! failure is an expected red (a known open finding reproduced on purpose).

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Pass,
    /// a known open finding, reproduced as designed (not a gate failure)
    ExpectedRed,
    /// an expected-red scenario that came out green: the finding may be fixed
    UnexpectedGreen,
    /// measured, no bound
    Info,
    Fail,
}

impl Status {
    pub fn word(&self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::ExpectedRed => "expected red",
            Status::UnexpectedGreen => "green (expected red)",
            Status::Info => "info",
            Status::Fail => "FAIL",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    pub scenario: String,
    pub invariant: String,
    pub status: Status,
    pub evidence: String,
}

pub struct Checks {
    scenario: String,
    rows: BTreeMap<String, Row>,
    order: Vec<String>,
    /// invariants whose failure is expected (known findings)
    pub expected_red: Vec<String>,
}

impl Checks {
    pub fn new(scenario: &str) -> Checks {
        Checks { scenario: scenario.into(), rows: BTreeMap::new(), order: Vec::new(), expected_red: Vec::new() }
    }

    /// Record a result; a failure overrides a pass of the same invariant, and
    /// the first failure's evidence is kept (with a count of repeats).
    pub fn add(&mut self, invariant: &str, mut status: Status, evidence: String) {
        if self.expected_red.iter().any(|e| e == invariant) {
            status = match status {
                Status::Fail => Status::ExpectedRed,
                Status::Pass => Status::UnexpectedGreen,
                s => s,
            };
        }
        match self.rows.get_mut(invariant) {
            Some(row) => {
                if status > row.status && status != Status::UnexpectedGreen {
                    row.status = status;
                    row.evidence = evidence;
                } else if status == row.status && matches!(status, Status::Fail | Status::ExpectedRed) {
                    if !row.evidence.contains("(+") {
                        row.evidence.push_str(" (+1 more)");
                    } else if let Some(i) = row.evidence.rfind("(+") {
                        let n: u64 = row.evidence[i + 2..].split(' ').next().and_then(|x| x.parse().ok()).unwrap_or(1);
                        row.evidence.truncate(i);
                        row.evidence.push_str(&format!("(+{} more)", n + 1));
                    }
                }
            }
            None => {
                self.order.push(invariant.to_string());
                self.rows.insert(invariant.to_string(), Row { scenario: self.scenario.clone(), invariant: invariant.into(), status, evidence });
            }
        }
    }

    pub fn pass(&mut self, invariant: &str, evidence: &str) {
        self.add(invariant, Status::Pass, evidence.into());
    }

    pub fn fail(&mut self, invariant: &str, evidence: &str) {
        self.add(invariant, Status::Fail, evidence.into());
    }

    pub fn info(&mut self, invariant: &str, evidence: &str) {
        self.add(invariant, Status::Info, evidence.into());
    }

    /// `ok` decides pass or fail.
    pub fn expect(&mut self, invariant: &str, ok: bool, evidence: String) {
        self.add(invariant, if ok { Status::Pass } else { Status::Fail }, evidence);
    }

    /// Mark invariants that never ran as passing when the absence of a failure is the pass.
    pub fn pass_if_quiet(&mut self, invariant: &str, evidence: &str) {
        if !self.rows.contains_key(invariant) {
            self.add(invariant, Status::Pass, evidence.into());
        }
    }

    pub fn rows(&self) -> Vec<Row> {
        self.order.iter().map(|k| self.rows[k].clone()).collect()
    }

    pub fn failed(&self) -> bool {
        self.rows.values().any(|r| r.status == Status::Fail)
    }
}
