//! The economy's checks, recomputed independently from the final main
//! chain: per-card stake at every boundary from the transactions alone,
//! Δs per epoch, the 1 % cap, the budget rule, every node's epoch records
//! equal, and the strategies' payoffs against chain-economy §9.

use std::collections::BTreeMap;

use soft3::chain::net::chain_view::ChainView;
use soft3::chain::tx::{Body, Tx};
use soft3::wallet::tx::{axon, card_id};

use crate::agents::Position;
use crate::check::Status;
use crate::genesis::Account;
use crate::world::World;

pub type Id = [u8; 32];

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Clone, Debug, Default)]
pub struct Payout {
    pub card: String,
    pub owner: String,
    pub amount: u64,
}

#[derive(Clone, Debug)]
pub struct Record {
    pub epoch: u64,
    pub digest: String,
    pub budget: u64,
    pub paid: u64,
    pub pool_after: u64,
    pub syntropy: i64,
    pub delta: i64,
    pub focus_bps: u64,
    pub payouts: Vec<Payout>,
}

pub fn records(w: &World, node: usize) -> Vec<Record> {
    let n = &w.nodes[node];
    let last = n.chain().economy(|c| Some(c.economy_status())).and_then(|v| v["last_settled"].as_u64()).unwrap_or(0);
    (1..=last)
        .filter_map(|e| n.chain().economy(|c| c.epoch_json(e)))
        .map(|v| Record {
            epoch: v["epoch"].as_u64().unwrap_or(0),
            digest: v["digest"].as_str().unwrap_or("").to_string(),
            budget: v["budget"].as_u64().unwrap_or(0),
            paid: v["paid"].as_u64().unwrap_or(0),
            pool_after: v["pool_after"].as_u64().unwrap_or(0),
            syntropy: v["syntropy"].as_i64().unwrap_or(0),
            delta: v["delta_syntropy"].as_i64().unwrap_or(0),
            focus_bps: v["focus_bps"].as_u64().unwrap_or(0),
            payouts: v["payouts"]
                .as_array()
                .map(|a| a.iter().map(|p| Payout { card: p["card"].as_str().unwrap_or("").into(), owner: p["owner"].as_str().unwrap_or("").into(), amount: p["amount"].as_u64().unwrap_or(0) }).collect())
                .unwrap_or_default(),
        })
        .collect()
}

/// Card stakes on the main chain of `node` at every height where they change.
pub fn stake_history(w: &World, node: usize) -> BTreeMap<Id, Vec<(u64, u64)>> {
    let n = &w.nodes[node];
    let tip = n.tip().height;
    let mut stake: BTreeMap<Id, u64> = BTreeMap::new();
    let mut hist: BTreeMap<Id, Vec<(u64, u64)>> = BTreeMap::new();
    for h in 1..=tip {
        let Some((_, body)) = n.chain().block_at(h) else { continue };
        for raw in n.chain().txs_of(&body).iter().skip(1) {
            let Ok(tx) = Tx::decode(raw) else { continue };
            match tx.body {
                Body::Cyberlink { from, amount, p, q, .. } if amount > 0 => {
                    let c = card_id(&from, &p, &q);
                    let s = stake.entry(c).or_insert(0);
                    *s += amount;
                    hist.entry(c).or_default().push((h, *s));
                }
                Body::Unlock { from, p, q, amount, .. } => {
                    let c = card_id(&from, &p, &q);
                    let s = stake.entry(c).or_insert(0);
                    *s = s.saturating_sub(amount);
                    hist.entry(c).or_default().push((h, *s));
                }
                _ => {}
            }
        }
    }
    hist
}

fn stake_at(hist: &[(u64, u64)], h: u64) -> u64 {
    hist.iter().take_while(|(x, _)| *x <= h).last().map(|x| x.1).unwrap_or(0)
}

/// Every economy check of a run.
pub fn analyze(w: &mut World, accounts: &[Account], positions: &[Position], length: u64) {
    let honest = w.honest_online();
    let Some(&obs) = honest.first() else { return };
    let recs = records(w, obs);
    w.metrics.emit(serde_json::json!({"kind": "economy", "epochs": recs.iter().map(|r| serde_json::json!({
        "epoch": r.epoch, "budget": r.budget, "paid": r.paid, "pool_after": r.pool_after, "syntropy": r.syntropy, "delta": r.delta,
        "focus_bps": r.focus_bps, "rewarded": r.payouts.len(), "digest": r.digest})).collect::<Vec<_>>()}));
    if recs.is_empty() {
        w.checks.fail("economy: settlements happened", "no epoch was settled");
        return;
    }
    w.checks.pass("economy: settlements happened", &format!("{} epochs settled, {} paid in total", recs.len(), recs.iter().map(|r| r.paid).sum::<u64>()));
    // determinism: every honest node's records equal
    let mut mismatch = Vec::new();
    for &n in &honest[1..] {
        let other = records(w, n);
        for r in &recs {
            if let Some(o) = other.iter().find(|o| o.epoch == r.epoch) {
                if o.digest != r.digest {
                    mismatch.push(format!("{} epoch {}", w.nodes[n].name, r.epoch));
                }
            }
        }
    }
    w.checks.expect("determinism: epoch records equal on every node", mismatch.is_empty(), if mismatch.is_empty() { format!("{} nodes × {} epochs", honest.len(), recs.len()) } else { mismatch.join(", ") });
    // budget rule and P ≤ B ≤ F
    let mut bad = Vec::new();
    for r in &recs {
        let f = r.paid + r.pool_after;
        if r.paid > r.budget || r.budget != f / soft3::chain::params::POOL_RELEASE {
            bad.push(format!("epoch {}: paid {} budget {} pool {}", r.epoch, r.paid, r.budget, f));
        }
        let sum: u64 = r.payouts.iter().map(|p| p.amount).sum();
        if sum != r.paid {
            bad.push(format!("epoch {}: Σ payouts {} ≠ paid {}", r.epoch, sum, r.paid));
        }
    }
    w.checks.expect("economy: P ≤ B = ⌊F/4⌋, Σ payouts = P", bad.is_empty(), if bad.is_empty() { format!("{} records", recs.len()) } else { bad.join("; ") });
    // the 1 % cap, from stake changes recomputed off the chain
    let hist = stake_history(w, obs);
    let mut over = Vec::new();
    let mut checked = 0;
    for r in &recs {
        let (b0, b1) = ((r.epoch - 1) * length, r.epoch * length);
        for p in &r.payouts {
            let Some((card, h)) = hist.iter().find(|(c, _)| hex(*c) == p.card) else {
                over.push(format!("epoch {} pays unknown card {}", r.epoch, &p.card[..8]));
                continue;
            };
            let _ = card;
            let ds = stake_at(h, b1).saturating_sub(stake_at(h, b0));
            let cap = (ds as u128 * soft3::chain::params::R_MAX_PPM as u128 / 1_000_000) as u64;
            checked += 1;
            if p.amount > cap {
                over.push(format!("epoch {} card {}: paid {} > cap {} (Δs {ds})", r.epoch, &p.card[..8], p.amount, cap));
            }
        }
    }
    w.checks.expect("economy: pay(c) ≤ 1 % of Δs(c)", over.is_empty(), if over.is_empty() { format!("{checked} payouts within cap") } else { over.join("; ") });

    // strategies
    let paid_to = |label_prefix: &str| -> BTreeMap<u64, u64> {
        let mut by_epoch = BTreeMap::new();
        for pos in positions.iter().filter(|p| p.label.starts_with(label_prefix)) {
            let c = hex(&card_id(&accounts[pos.account].neuron, &pos.p, &pos.q));
            for r in &recs {
                for p in r.payouts.iter().filter(|p| p.card == c) {
                    *by_epoch.entry(r.epoch).or_insert(0) += p.amount;
                }
            }
        }
        by_epoch
    };
    let total = |m: &BTreeMap<u64, u64>| m.values().sum::<u64>();
    let mut summary = serde_json::Map::new();
    for prefix in ["honest", "concentrate", "recycle", "hedge+", "hedge-", "sybil-one", "sybil-split", "ring", "original", "frontrun", "grief", "payout"] {
        let m = paid_to(prefix);
        summary.insert(prefix.into(), serde_json::json!(m.iter().map(|(e, a)| format!("{e}:{a}")).collect::<Vec<_>>()));
    }
    w.metrics.emit(serde_json::json!({"kind": "economy-strategies", "paid": summary}));

    // recycler: the relock must not earn more than the first lock (§9 "yield ≤ the mark's decay")
    if let Some(rec) = positions.iter().find(|p| p.label.starts_with("recycle")) {
        let m = paid_to("recycle");
        let first = m.get(&rec.epoch).copied().unwrap_or(0);
        let later: u64 = m.iter().filter(|(e, _)| **e > rec.epoch).map(|(_, a)| *a).sum();
        let ev = format!("first lock paid {first}, relock paid {later} (epochs {:?})", m);
        w.checks.add("economy §9: recycler earns ≤ the mark's decay", if later <= first / 50 { Status::Pass } else { Status::Fail }, ev);
    }
    // hedger: equal +1 / −1 on one axon leave A unchanged and must earn nothing
    let (hp, hm) = (total(&paid_to("hedge+")), total(&paid_to("hedge-")));
    if positions.iter().any(|p| p.label.starts_with("hedge")) {
        w.checks.add("economy §9: hedge (+1/−1 equal) earns nothing", if hp + hm == 0 { Status::Pass } else { Status::Fail }, format!("+1 side paid {hp}, −1 side paid {hm}"));
    }
    // Sybil: one key vs the same stake split over many keys on the same axon side
    if positions.iter().any(|p| p.label.starts_with("sybil")) {
        let one = total(&paid_to("sybil-one"));
        let split = total(&paid_to("sybil-split"));
        let n = positions.iter().filter(|p| p.label.starts_with("sybil-split")).count() as u64;
        w.checks.expect("economy §9: Sybil splitting changes no total", one.abs_diff(split) <= n + 1, format!("one key {one}, split over {n} keys {split}"));
    }
    // ring: paid at most 1 % of its stake per settlement (cap) — covered above; report
    if positions.iter().any(|p| p.label.starts_with("ring")) {
        let ring = total(&paid_to("ring"));
        let staked: u64 = positions.iter().filter(|p| p.label.starts_with("ring")).map(|p| p.amount).sum();
        w.checks.expect("economy §9: self-linking ring ≤ 1 % of its stake per epoch", ring as u128 * 100 <= staked as u128 * (recs.len() as u128), format!("ring of {} positions, stake {staked}, paid {ring} over {} epochs", positions.iter().filter(|p| p.label.starts_with("ring")).count(), recs.len()));
    }
    // last-block copy: pro rata with the original, never more
    if positions.iter().any(|p| p.label.starts_with("frontrun")) {
        let (o, f) = (total(&paid_to("original")), total(&paid_to("frontrun")));
        w.checks.expect("economy §9: last-block copy earns ≤ the original", f <= o, format!("original {o}, late copy {f}"));
    }
    // payout owner: the reward goes to the owner in G_E
    for pos in positions.iter().filter(|p| p.payout_to.is_some()) {
        let c = hex(&card_id(&accounts[pos.account].neuron, &pos.p, &pos.q));
        let author = hex(&accounts[pos.account].neuron);
        let new_owner = hex(&accounts[pos.payout_to.unwrap()].neuron);
        let mut seen = Vec::new();
        for r in &recs {
            for p in r.payouts.iter().filter(|p| p.card == c) {
                seen.push((r.epoch, if p.owner == author { "author" } else if p.owner == new_owner { "new owner" } else { "other" }));
            }
        }
        let ok = seen.iter().all(|(e, who)| (*e <= pos.epoch && *who == "author") || (*e > pos.epoch && *who == "new owner"));
        w.checks.expect("economy §3.5: paid to the payout owner of G_E", ok, format!("{} lock epoch {}, owner changed after its boundary; paid {:?}", pos.label, pos.epoch, seen));
    }
    // griefer: −1 against the dominant edge — report
    if positions.iter().any(|p| p.label.starts_with("grief")) {
        let g = total(&paid_to("grief"));
        w.checks.info("economy §9: −1 griefer payoff", &format!("paid {g} (earns only if zeroing raised syntropy)"));
    }
    let conc = total(&paid_to("concentrate"));
    let hon = total(&paid_to("honest"));
    w.checks.info("economy: concentration vs diverse honest stakers", &format!("concentrator paid {conc}; honest diverse stakers paid {hon}"));
    let _ = axon;
}
