---
title: the phase-1 chain under simulation — receipt 2026-10-18
tags: cyber, soft3, simulation, audit, chain, network, economy, launch, security
crystal-type: audit
crystal-domain: cyber
date: 2026-10-18
---

# the phase-1 chain under simulation — receipt 2026-10-18

The owner's order of 2026-10-17: besides running the network, build the
simulator into delivery and check what works and what does not — the
network must take off, must not hang, and must be secure. Launch is
2026-11-05 (pussy 13:37 UTC, bostrom 13:22:42 UTC); code freeze 2026-11-02.

This receipt records what was built (2026-10-17/18), what was run, and
what was observed.
Requirements stay in soft3 `specs/chain.md`, `chain-wire.md`,
`chain-economy.md`; the simulator's design is `README.md` § chain.

## verdict

- **Takes off.** Both real genesis files open on every node with the
  pinned chain ids, block-0 ids and state roots; miners joining from zero
  hashrate mine the first block at the real 2^232 target, ASERT settles at
  30 s, both epochs settle on every node with equal records, a late joiner
  syncs from genesis through two settlements (`launch-pussy`,
  `launch-bostrom`).
- **Does not hang** — after eleven kernel/wire fixes found here and merged
  into soft3 main (table below). Before them: a released deep fork stalled
  honest nodes for 40 minutes and fragmented the honest set; a late joiner
  holding a checkpoint never downloaded a body; a transaction flood got
  honest relays banned and isolated. Bounded stalls that remain are the
  spec's own: a settlement that computes longer than D blocks stops the
  chain until it finishes (`settle-slow`), a deterministic panic of the φ*
  engine halts it (`settle-panic`), the first block after a hashrate
  collapse is mined at the old target (`asert-step`).
- **Secure, within the stated model** — every forged block with valid work
  was refused, every hostile frame kind banned its sender, conservation
  held at every block on every node, honest nodes never disagreed below
  depth 120, no reorganization ever exceeded 119, every run is
  bit-reproducible on macOS arm64 and Linux x86_64. Open, for the owner:
  **there are no checkpoints at launch** (both genesis files carry an
  all-zero key and no signer exists), so a released deep rewrite captures
  late joiners; the economy's three known findings (expected red, failing
  tests in soft3#234); and the cost of the reorg limit — an eclipse or
  partition longer than an hour strands nodes until an operator resyncs.

## what was run

| | |
|---|---|
| simulation | `cyberia-to/simulation` `b2e8bf2`, branch `feat/network-sim` (PR #1): the binaries of every suite below |
| soft3 | main `0c6cbb5` — every number in the suites below ran on it (it carries every fix in the table; the consensus rules are those of `d7a407e` but for the checkpoint handling of #233, #240 and #241) |
| stand | `stand.toml`: hemera release/0.4 `7237a23`, bbg `be5c5db`, tru `e660d1e`, lens `529adcb`, zheng `8f20262`, nox `12adaf8`, cybergraph `4c73149`, foculus `b28f0a2`, mudra `6a53ed3`, tok `c773592`, radio `949fad7c`, tade `778dc6f`, strata `12a7a6d`, inf `3a2c3f5`, neuron `9df467d`, file `ae9a57d`, honeycrisp `40861c7`, joy `10844a8`, trident `f96d0b5e`; `cyber-hemera "0.3" → "0.4"` stand-local in bbg, cybergraph, foculus, mudra, nox, tok, tru, zheng (the economy receipt's open item) |
| machine | Apple M4 Max, 48 GB, macOS 26.4.1; CI: GitHub `ubuntu-24.04` x86_64 |
| commands | `cargo build --profile sim -p simulation-chain` (and `--features metal` for `launch`); `simulation-chain gate --seed 1`; `simulation-chain nightly --seed 1`; `simulation-chain launch --seed 1` |
| outputs | per scenario JSONL (`/tmp/simulation-chain/final/<suite>/*.jsonl`, throwaway) and the verdict tables reproduced in the appendix |

| suite | scenarios | wall | verdict |
|---|---|---|---|
| `gate` (M4 Max) | 16 | 392 s | all green; 2 expected red (economy findings 1, 4) |
| `nightly` (M4 Max) | 11 | 1,183 s | all green; expected red: eclipse > 1 h, checkpoints never fetched (main before #239) |
| `launch` (M4 Max, Metal) | 2 | 1,757 s (pussy 881 s, bostrom 871 s) | all green; expected red: bostrom syntropy (finding 3) |
| `gate` on GitHub `ubuntu-24.04` | 16 | stand 13 s, build 4 m 47 s, suite 9 m 15 s | green; every scenario's digest equals the M4 Max's (2026-10-17 comparison, all 16) |

## how it works (short)

N in-process nodes, each the real kernel behind the wire's port
(`soft3::chain::net::kernel::KernelChain` over its own fjall store), the
real mempool, real chain-wire frames encoded and decoded by `net::frames`,
and the protocol of `net/gossip.rs`, `net/sync.rs`, `net/serve.rs`
re-expressed as event handlers on a virtual clock. A simulated network
(latency, jitter, loss, bandwidth queues, partitions, eclipses, crashes
with replay from disk, late joiners, clock skew). Miners find blocks after
an exponential time of mean `work(target) / hashrate`, then grind a
genuinely valid nonce that every node validates like any other block —
on test networks against a 2^248 genesis target with the `sim` feature's
relaxed `POW_LIMIT` (ASERT is scale-invariant), on the launch genesis files
against the real 2^232 target on the GPU. Agents send real signed
transactions; hostile peers send real frames that honest nodes score by
their own rules. Every map is ordered, the exponential sampler is integer
only: a seed reproduces a run bit for bit (`determinism`, and the CI's
digests equal the M4 Max's for every gate scenario).

Not simulated: QUIC streams and the iroh-gossip mesh (a request/response
pair, broadcast to the node's links, with iroh-gossip's message cache
modelled as "a repeat never reaches the application"), the per-peer token
buckets (they read the wall clock), discovery and the address book;
topologies are random meshes of degree 3–4, up to 24 nodes (bostrom: 3,
memory). Adversaries are simple: SM1 selfish mining, a private fork that
publishes once, an eclipser that is one synced node.

## soft3 bugs found and fixed (merged into main)

| PR | found by | what was wrong | fix | test |
|---|---|---|---|---|
| [#229](https://github.com/cyberia-to/soft3/pull/229) | `flood` | mempool admission replayed every pending predecessor through `execute_tx`, re-verifying their secp256k1 signatures; re-validation after every tip did it for the whole pool: 792 verifications for 16 transactions; eviction ties fell to `HashMap` order (per process) | a bounded cache of verified signatures; ties by txid | `a_signature_is_verified_once_however_often_the_pool_readmits_it` (red 792, green ≤ 16) |
| [#230](https://github.com/cyberia-to/soft3/pull/230) | `reorg-140` | after honest nodes refused a released fork 140 deep, the refused chain stayed the most-work header: body sync followed it and never fetched the honest best branch; an honest node on a stale branch stalled 40 min, the honest set fragmented | `Chain::best_reachable`: the heaviest header the node may still move to | `a_refused_deep_fork_does_not_capture_body_sync` (red: `missing_bodies` empty) |
| [#231](https://github.com/cyberia-to/soft3/pull/231) | `flood` | the mempool ordered and evicted by integer `fee / len`: every fee below twice the floor ranked with the floor (40 % of the honest premium transactions never mined, p90 wait 1,135 s); a stale nonce (already mined) cost the relaying peer +10 — honest relays banned within 25 min | exact cross-product comparison; `AdmitError::Stale`, score 0 | `fee_rate_is_compared_exactly`, `a_stale_nonce_is_not_scored` (red first) |
| [#232](https://github.com/cyberia-to/soft3/pull/232) | `flood` | a relayed nonce beyond the receiver's window (the sender's tip holds blocks the receiver lacks) cost +10: a busy account fills its 16-nonce window every block, so every block race charged honest peers | +1 like a duplicate, 0 from a peer that announced a higher tip; chain-wire §5 amended (superseded by #238) | `a_relayed_nonce_beyond_our_window_is_a_race_not_an_offence` |
| [#233](https://github.com/cyberia-to/soft3/pull/233) | `late-join` | a node whose tip is below a held checkpoint treated the checkpointed chain itself as "below the floor": since #230 a late joiner that fetched the foundation checkpoint before its headers never asked for a body (height 0 forever); before, a top-body-first sync answered `Checkpoint` instead of `NeedBodies` | the floor binds only once the tip has reached it (`try_advance`, `own_chain_blocked`) | `a_fresh_node_holding_a_checkpoint_downloads_through_it` (red: 0 bodies) |
| [#236](https://github.com/cyberia-to/soft3/pull/236) | `flood` on CI | re-validating a full pool (8,192 transactions) took 3,121 ms per tip on the Linux runner: k(k+1)/2 executions for an account's run of k, a hemera statement per cached signature | one scratch pass per account (`ChainView::validate_run`); cache keyed by `sha256(chain ‖ bytes)` | `revalidation_in_one_pass_per_account_equals_readmission`; upkeep 1,237 → 154 ms, flood digest unchanged |
| [#237](https://github.com/cyberia-to/soft3/pull/237) | `partition-long` | header-first relay races the relayer's own body fetch; `fetch_body` charged the announcer +1 when it could not serve yet (not in chain-wire §5): at two blocks a minute against a decay of one point a minute an honest node banned its only same-side peer and stalled 31 min | no charge; a sync hint | scenario evidence (async path) |
| [#238](https://github.com/cyberia-to/soft3/pull/238) | `flood-long` | after #232 a relayed nonce beyond the window still cost +1: a hub relaying for many busy accounts sends hundreds per lagging block — an honest relay was banned after 99 min of flood; the charge bought nothing (an unfundable transaction costs the same check at 0) | never charged; chain-wire §5 amended (the per-peer transaction rate bounds the cost) | `a_relayed_nonce_beyond_our_window_is_a_race_not_an_offence`; `flood-long`: 0 points charged among honest nodes |
| [#239](https://github.com/cyberia-to/soft3/pull/239) | `reorg-140-checkpoints-unfetched` | the wire answered `k` (get-checkpoints) but never sent it, and checkpoints live in memory: a late joiner or a restarted node held no checkpoint for up to an hour and followed a released 140-deep rewrite (60 % hashrate) | `k` on every new connection (chain-wire §3.1 step 1); `Net::publish_checkpoint` | `tests/chain_checkpoints_on_connect.rs`: two in-process nodes (red: none after 30 s) |
| [#240](https://github.com/cyberia-to/soft3/pull/240) | `reorg-140-checkpoints` | a held checkpoint did not stop a block at its height that extended the tip directly (steps 1–5 never consulted it), nor move a node off a main chain that contradicts a checkpoint received later (fork choice only looks at heavier chains; `accept_checkpoint` only logged) | checkpoint checked in `check_header`; `Chain::reject_main_from` retracts and moves within the reorg limit | `a_block_that_contradicts_a_held_checkpoint_is_refused_even_on_the_tip`, `a_checkpoint_that_contradicts_the_main_chain_moves_the_node_to_the_checkpointed_chain` (red first) |
| [#241](https://github.com/cyberia-to/soft3/pull/241) | `partition-long` (after #240) | blocks a node invalidates for contradicting a checkpoint — and their children — cost the relaying peer +10 (`InvalidParent`, `Rejected`): honest minority neighbours that had not yet seen the checkpoint were banned after a long partition healed | such blocks are *vetoed*: refused as `Checkpoint`, never scored | the #240 test extended (red: `InvalidParent`) |

Enabling change: [#228](https://github.com/cyberia-to/soft3/pull/228)
(`sim` feature: process-wide relaxed `POW_LIMIT` for test genesis files;
`KernelChain::open_with`). Every PR: `cargo test --release
--no-default-features --features chain-conformance` green on the stand
(lib 276 → 285 passed, 1 ignored; 17 bin, 2 adversary, 1
checkpoints-on-connect, 1 mine e2e).

## open findings, reproduced (expected red)

The economy's known findings (`cybernode/audit/chain/economy-2026-10-17.md`
§ findings), run as expected-red rows here and as failing kernel tests in
draft [soft3#234](https://github.com/cyberia-to/soft3/pull/234) — not
fixed: attribution and the record are the economy's decisions.

| finding | simulator | kernel test (red) |
|---|---|---|
| 1 · a quiet-epoch relock earns more than the first lock (§9 says "≤ the mark's decay") | first lock paid 43704, relock paid 1093585 (by epoch {2: 43704, 5: 1093585}; relock the only earner in epochs [5]) | first lock 3,757,610, quiet relock 10,333,427 |
| 4 · an equal +1/−1 hedge is credited (§9 says attributions cancel) | +1 side paid 71569, −1 side paid 0 | +1 side 5,315,353, −1 side 0 |
| 3 · the record's `syntropy: i64` saturates on bostrom | epoch 1 syntropy 9223372036854775807; epoch 2 syntropy 9223372036854775807 | J(φ*₀) 9,886,742,015,786,730,065 recorded as 9,223,372,036,854,775,807 |

## for the owner — properties measured, not bugs

1. **There are no checkpoints at launch.** The pussy and bostrom genesis
   files carry `checkpoint_key` all-zero (specs/genesis.md: "no key set
   yet"), so checkpoints are disabled, and soft3 has no signer — nothing
   outside tests signs one. chain.md §5.3 calls them "the explicit,
   temporary mitigation" for a hash rate one GPU farm can out-mine; the
   simulator shows what that means: with checkpoints off, a late joiner
   follows a released 140-deep rewrite of a 60 % attacker
   (`reorg-140`: late joiner at 490 follows the attacker's chain; checkpoints off); with checkpoints fetched it does not
   (`reorg-140-checkpoints`). The wire now fetches and enforces them
   (#239, #240); a key in the genesis (a new chain id, before the freeze)
   and a signer process are the owner's decision.
2. **An eclipse or partition longer than ~1 h strands nodes for good.**
   The reorg limit (chain.md §5.2) is working as written; the price is
   that an eclipser with **5 % of the hashrate** grew a private branch of
   292 blocks in 2 h 30 (ASERT eases its own difficulty), and
   the victim could never rejoin (eclipse 150 min by a 5 % miner: attacker chain grew 292 blocks; victim at 239 (stuck on the attacker's branch), honest at 460). A 30-minute eclipse heals
   (`eclipse`). A 2-hour partition leaves the minority on its own chain
   until an operator wipes and resyncs, which the simulator does and which
   then converges (`partition-long`). Checkpoints protect *late joiners*
   (`reorg-140-checkpoints`) and, since #240, move a node off a
   contradicted chain within the reorg limit — not a node whose own branch
   is already deeper than 119 (the eclipse victim). Options: an operator
   resync from the latest checkpoint (the runbook), the sync-quorum gate
   for relays as well as miners, or a checkpoint that may override the
   reorg limit while checkpoints are honored.
3. **A slow settlement stops the chain, by design.** With the settlement
   computing 25 min against D = 20 blocks (10 min), every node waited at
   `settle(E)` (longest gap 24 min) and resumed; nothing forked.
   With the spec's clock that is D = 120 blocks = 1 h against the
   measured max 52003 ms; n0:e2 52003 ms (2990817 axons), n0:e1 44647 ms (2990817 axons), n1:e2 51090 ms (2990817 axons), n1:e1 44925 ms (2990817 axons), n2:e2 50383 ms (2990817 axons), n2:e1 37165 ms (2990817 axons) for bostrom — a wide margin.
4. **A panicking φ* engine halts the chain.** `Economy::result` joins the
   worker with `expect`; every node crashed at `settle(1)`, a restart
   replays to the same block and crashes again. Correct per §2.5 (never
   guess), but it makes tru's robustness a liveness property of the chain.
5. **The first block after a hashrate collapse is mined at the old
   target** (ASERT reads the parent only): ÷10 → first block 572 s after the step (exponential, mean 300 s: P(> 1 h) = e^(−3600/300)), longest gap 761 s; ÷100 →
   first block 6929 s after the step (exponential, mean 3000 s: P(> 1 h) = e^(−3600/3000)), longest gap 6957 s. Recovery to 30 s ± 20 %: ÷10 2887 s after the step (window 9000 s); ÷100 8613 s after the step (window 19200 s).
6. **Selfish mining** (SM1, attacker linked to every honest node with
   10 ms links): α = 33 %: 30.7 % of 355 main-chain blocks (hashrate 33.0 %); deepest honest reorg 3; α = 40 %: 43.5 % of 950 main-chain blocks (hashrate 40.0 %); deepest honest reorg 11. Below a third it does not pay here; at 40 %
   it does (the textbook result) — Nakamoto's bound, not a soft3 defect;
   the reorgs stay shallow and every node stays live.
7. **A ban isolates a node for an hour — the finality window.** Before
   #232/#238, flood scoring banned honest peers; one ban wave cut the
   flood's entry node (which also hosts the 40 % miner) off from every
   peer for `BAN_SECS` = 1 h, it mined alone and forked deeper than 119
   — a permanent split from a scoring mistake. The scoring is fixed, but
   the shape remains: anything that bans a miner's honest peers for an
   hour partitions the chain for good. Worth a rule that a node keeps at
   least one honest outbound peer (anchors/bootstrap never banned below
   +100-for-proof offences), or a ban shorter than the reorg window.
8. **Concentration pays.** Under J = D_KL(φ ‖ u), in the `economy` run:
   concentrator paid 285656; honest diverse stakers paid 27966 (the economy receipt's finding 2).
9. **Catching up recomputes every settlement.** At the tip a bostrom
   settlement costs nothing on the block path (≤ 11 ms per block; the
   worker finishes in 37–52 s within the D = 120 blocks).
   A node syncing history computes each one inline at `settle(E)`: the
   bostrom late joiner spent up to 37 s on one block, and every epoch of
   history costs up to ~52 s on 4 threads — 12 epochs a day, so a month
   of history is ~5 h and a year ~2.5 days of settlement compute before a
   new node reaches the tip. A shared settlement cache, or an economy
   state committed in checkpoints, would bound it; phase-1 scale makes it
   a near-term question, not a launch blocker.

## delivery

- **CI** (`cyberia-to/simulation` `.github/workflows/gate.yml`): the gate
  suite on Linux on every push and pull request, on every soft3 merge
  (`repository_dispatch soft3-merged`, sent by soft3's
  `notify-simulation.yml`, [#235](https://github.com/cyberia-to/soft3/pull/235)),
  and every six hours against soft3 main. git, python3 (stdlib) and cargo
  only: no actions, no Node. The stand is rebuilt from `stand.toml` by
  `scripts/stand.py`. Latest green run before this receipt: `39082233075` (simulation `b2e8bf2`, soft3 main).
- **Release train**: `simulation-gate` in `soft3/release/train_gates.py`
  among the stack gates ([#235](https://github.com/cyberia-to/soft3/pull/235)),
  origin-pinned like every gate, verdict table beside the receipts as
  `audit/release-<date>/simulation-gate.md`; named in `specs/releases.md`
  and soft3 `CLAUDE.md`. Blocked until the simulation revision is pinned in
  `release/phase1.toml` — that pin is a bump PR left for the owner
  (a bump PR adding `cyberia-to/simulation` to `soft3/release/phase1.toml`, opened once this receipt is on simulation main). Until the hemera-0.4 sweep reaches the siblings' default
  branches the train's own checkouts do not build soft3 (the economy
  receipt's open item), so this gate shares that blocker.
- **Tracker**: `cyber/launch.md` property row (a one-row pull request to cyberia-to/cyber, row 46).

## suites

### gate

| scenario | virtual | wall | blocks | pass | fail | expected red | info | digest |
|---|---|---|---|---|---|---|---|---|
| steady | 2h37m | 20.3 s | 304 | 14 | 0 | 0 | 1 | `eb088c0afd84` |
| determinism | 0h53m | 15.9 s | 100 | 2 | 0 | 0 | 0 | `b79123e413fd` |
| partition-heal | 1h33m | 13.8 s | 172 | 12 | 0 | 0 | 1 | `2fb9023e25ad` |
| crash-restart | 2h06m | 16.7 s | 243 | 11 | 0 | 0 | 2 | `eb0c8153ff67` |
| late-join | 3h07m | 26.8 s | 365 | 11 | 0 | 0 | 2 | `c8b53c0c4240` |
| clock-skew | 2h07m | 16.6 s | 238 | 12 | 0 | 0 | 2 | `ed5150a0d8a5` |
| asert-step | 9h06m | 43.4 s | 1089 | 13 | 0 | 0 | 2 | `045b30ff0757` |
| selfish | 3h10m | 21.8 s | 357 | 12 | 0 | 0 | 2 | `b892c0829e26` |
| reorg-60 | 2h46m | 20.3 s | 351 | 11 | 0 | 0 | 1 | `c9f6a342fb85` |
| reorg-140 | 4h09m | 62.1 s | 481 | 11 | 0 | 0 | 2 | `7be4212b1340` |
| eclipse | 1h58m | 11.1 s | 212 | 11 | 0 | 0 | 1 | `ce414d5aaa33` |
| flood | 0h56m | 45.2 s | 97 | 13 | 0 | 0 | 2 | `9ee3d25ffbcf` |
| hostile | 2h07m | 22.8 s | 243 | 33 | 0 | 0 | 1 | `cd5d098b8ccf` |
| economy | 3h02m | 17.5 s | 353 | 19 | 0 | 2 | 3 | `67cbf2f69093` |
| settle-slow | 4h21m | 19.6 s | 405 | 12 | 0 | 0 | 1 | `51da0822cd60` |
| settle-panic | 2h00m | 2.7 s | 60 | 0 | 0 | 0 | 1 | `` |

### nightly

| scenario | virtual | wall | blocks | pass | fail | expected red | info | digest |
|---|---|---|---|---|---|---|---|---|
| steady-large | 8h07m | 190.9 s | 960 | 14 | 0 | 0 | 1 | `df67e76a282c` |
| partition-long | 3h35m | 52.5 s | 414 | 11 | 0 | 0 | 2 | `2d42ecbf7c51` |
| asert-100x | 11h57m | 74.8 s | 1425 | 13 | 0 | 0 | 2 | `11142f36a8d2` |
| selfish-40 | 8h08m | 58.9 s | 952 | 12 | 0 | 0 | 2 | `9d9e72a63079` |
| reorg-140-checkpoints | 4h06m | 61.0 s | 475 | 12 | 0 | 0 | 1 | `d60674e2b166` |
| reorg-140-checkpoints-unfetched | 4h11m | 63.0 s | 482 | 11 | 0 | 1 | 1 | `1b2caa926bfb` |
| eclipse-long | 3h56m | 32.6 s | 462 | 10 | 0 | 1 | 1 | `da01af04060f` |
| forged-header-storm | 5h08m | 44.5 s | 598 | 33 | 0 | 0 | 1 | `69111bef4f33` |
| flood-long | 2h47m | 499.8 s | 318 | 13 | 0 | 0 | 2 | `d7740d3ced6d` |
| economy-long | 8h23m | 47.3 s | 987 | 19 | 0 | 2 | 3 | `8b0562425b15` |
| settle-slow-one | 4h25m | 43.7 s | 477 | 12 | 0 | 0 | 1 | `848f46aee0dd` |

### launch

| scenario | virtual | wall | blocks | pass | fail | expected red | info | digest |
|---|---|---|---|---|---|---|---|---|
| launch-pussy | 5h18m | 881.3 s | 620 | 17 | 0 | 0 | 4 | `3c4bb0bc2bf7` |
| launch-bostrom | 5h16m | 871.3 s | 623 | 16 | 0 | 1 | 4 | `ddb1b51b9bbb` |

## appendix — verdict tables

### gate

| scenario | invariant | verdict | evidence |
|---|---|---|---|
| steady | liveness: mean block interval 30 s ± 20 % | pass | 281 blocks, mean 29.9 s, p99 124 s, max 238 s (bound 24–36 s) |
| steady | stale blocks ≤ 5 % | pass | 0 of 302 found blocks off the main chain |
| steady | safety: converge after quiesce | pass | 8 honest nodes at height 304 (1a6c390b) |
| steady | honest traffic: honest txs mined | pass | 2213/2213 mined (100 %), wait p50 20 s, p90 69 s, max 234 s; 0 refused at submit (bound ≥ 90 %) |
| steady | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 2432 block applications on 8 nodes |
| steady | no node panicked | pass | none |
| steady | resource: per-block apply time ≤ 2 s | pass | p99 8 ms, max 12 ms over 2437 applications; template max 4 ms |
| steady | no honest peer banned | pass | none |
| steady | resource: mempool upkeep per tip move ≤ 2 s | pass | max 0 ms (remove mined + re-admit the pool) |
| steady | resource: peak RSS | info | 49 MB (process) |
| steady | resource: mempool ≤ capacity | pass | max 53 txs |
| steady | liveness: no stall | pass | no honest node waited longer than the stall bound |
| steady | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| steady | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| steady | checkpoints reach every node | pass | 1 checkpoints held by node 1 |
| determinism | determinism: same seed ⇒ bit-identical run | pass | digest b79123e413fdffba vs b79123e413fdffba (height 100 vs 100) |
| determinism | determinism: another seed ⇒ another run | pass | seed+1 digest 602358661727edab |
| partition-heal | partition < 1 h heals by reorg | pass | minority reorg depths [9, 1, 13, 9, 1, 9] |
| partition-heal | safety: converge after quiesce | pass | 10 honest nodes at height 172 (333f9d91) |
| partition-heal | traffic: honest txs mined | pass | 858/929 mined (92 %), wait p50 26 s, p90 587 s, max 1738 s; 0 refused at submit (bound ≥ 90 %) |
| partition-heal | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 1765 block applications on 10 nodes |
| partition-heal | no node panicked | pass | none |
| partition-heal | resource: per-block apply time ≤ 2 s | pass | p99 9 ms, max 24 ms over 1774 applications; template max 16 ms |
| partition-heal | no honest peer banned | pass | none |
| partition-heal | resource: mempool upkeep per tip move ≤ 2 s | pass | max 4 ms (remove mined + re-admit the pool) |
| partition-heal | resource: peak RSS | info | 53 MB (process) |
| partition-heal | resource: mempool ≤ capacity | pass | max 104 txs |
| partition-heal | liveness: no stall | pass | no honest node waited longer than the stall bound |
| partition-heal | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| partition-heal | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| crash-restart | crash: restart replays from disk | info | 0 slow opens; nodes 0, 1, 6, 7 restarted |
| crash-restart | safety: converge after quiesce | pass | 8 honest nodes at height 243 (9dd8c8b2) |
| crash-restart | traffic: honest txs mined | pass | 1455/1455 mined (100 %), wait p50 21 s, p90 66 s, max 129 s; 0 refused at submit (bound ≥ 90 %) |
| crash-restart | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 1961 block applications on 8 nodes |
| crash-restart | no node panicked | pass | none |
| crash-restart | resource: per-block apply time ≤ 2 s | pass | p99 8 ms, max 15 ms over 1973 applications; template max 4 ms |
| crash-restart | no honest peer banned | pass | none |
| crash-restart | resource: mempool upkeep per tip move ≤ 2 s | pass | max 1 ms (remove mined + re-admit the pool) |
| crash-restart | resource: peak RSS | info | 54 MB (process) |
| crash-restart | resource: mempool ≤ capacity | pass | max 35 txs |
| crash-restart | liveness: no stall | pass | no honest node waited longer than the stall bound |
| crash-restart | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| crash-restart | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| late-join | late joiners: initial block download | info | late0@363:e0537467, late1@363:e0537467, late2@363:e0537467 |
| late-join | safety: converge after quiesce | pass | 9 honest nodes at height 365 (ec01f25b) |
| late-join | traffic: honest txs mined | pass | 2115/2115 mined (100 %), wait p50 21 s, p90 63 s, max 172 s; 0 refused at submit (bound ≥ 90 %) |
| late-join | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 3296 block applications on 9 nodes |
| late-join | no node panicked | pass | none |
| late-join | resource: per-block apply time ≤ 2 s | pass | p99 12 ms, max 95 ms over 3317 applications; template max 8 ms |
| late-join | no honest peer banned | pass | none |
| late-join | resource: mempool upkeep per tip move ≤ 2 s | pass | max 3 ms (remove mined + re-admit the pool) |
| late-join | resource: peak RSS | info | 60 MB (process) |
| late-join | resource: mempool ≤ capacity | pass | max 36 txs |
| late-join | liveness: no stall | pass | no honest node waited longer than the stall bound |
| late-join | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| late-join | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| clock-skew | clock skew ±200 s: future-drift holds | info | 99 headers or blocks held for the 120 s drift rule, re-evaluated on the tick |
| clock-skew | liveness under skew: mean interval 30 s ± 25 % | pass | 209 blocks, mean 31.3 s, p99 91 s, max 146 s (bound 22–38 s) |
| clock-skew | safety: converge after quiesce | pass | 8 honest nodes at height 238 (e88b240e) |
| clock-skew | traffic: honest txs mined | pass | 1175/1175 mined (100 %), wait p50 19 s, p90 61 s, max 125 s; 0 refused at submit (bound ≥ 90 %) |
| clock-skew | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 1987 block applications on 8 nodes |
| clock-skew | no node panicked | pass | none |
| clock-skew | resource: per-block apply time ≤ 2 s | pass | p99 9 ms, max 13 ms over 2071 applications; template max 5 ms |
| clock-skew | no honest peer banned | pass | none |
| clock-skew | resource: mempool upkeep per tip move ≤ 2 s | pass | max 2 ms (remove mined + re-admit the pool) |
| clock-skew | resource: peak RSS | info | 60 MB (process) |
| clock-skew | resource: mempool ≤ capacity | pass | max 54 txs |
| clock-skew | liveness: no stall | pass | no honest node waited longer than the stall bound |
| clock-skew | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| clock-skew | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| asert-step | ASERT ×10 spike: interval back to 30 s ± 20 % | pass | 1432 s after the step |
| asert-step | ASERT ÷10 collapse: interval back to 30 s ± 20 % | pass | 2887 s after the step (window 9000 s) |
| asert-step | ASERT ÷10: first block after the collapse, longest gap | info | first block 572 s after the step (exponential, mean 300 s: P(> 1 h) = e^(−3600/300)), longest gap 761 s |
| asert-step | liveness: steady again after the steps | pass | 258 blocks, mean 27.8 s, p99 122 s, max 251 s (bound 22–38 s) |
| asert-step | safety: converge after quiesce | pass | 6 honest nodes at height 1089 (4a6a7922) |
| asert-step | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 6534 block applications on 6 nodes |
| asert-step | no node panicked | pass | none |
| asert-step | resource: per-block apply time ≤ 2 s | pass | p99 7 ms, max 23 ms over 6559 applications; template max 0 ms |
| asert-step | no honest peer banned | pass | none |
| asert-step | resource: mempool upkeep per tip move ≤ 2 s | pass | max 5 ms (remove mined + re-admit the pool) |
| asert-step | resource: peak RSS | info | 68 MB (process) |
| asert-step | resource: mempool ≤ capacity | pass | max 0 txs |
| asert-step | liveness: no stall | pass | no honest node waited longer than the stall bound |
| asert-step | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| asert-step | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| selfish | selfish mining at α = 33 %: revenue share | info | 30.7 % of 355 main-chain blocks (hashrate 33.0 %); deepest honest reorg 3 |
| selfish | liveness under selfish mining | pass | 338 blocks, mean 30.1 s, p99 170 s, max 231 s (bound 22–40 s) |
| selfish | selfish: honest reorgs stay shallow | pass | deepest 3 |
| selfish | safety: converge after quiesce | pass | 8 honest nodes at height 357 (998ed132) |
| selfish | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 3405 block applications on 9 nodes |
| selfish | no node panicked | pass | none |
| selfish | resource: per-block apply time ≤ 2 s | pass | p99 6 ms, max 11 ms over 3441 applications; template max 0 ms |
| selfish | no honest peer banned | pass | none |
| selfish | resource: mempool upkeep per tip move ≤ 2 s | pass | max 0 ms (remove mined + re-admit the pool) |
| selfish | resource: peak RSS | info | 69 MB (process) |
| selfish | resource: mempool ≤ capacity | pass | max 0 txs |
| selfish | liveness: no stall | pass | no honest node waited longer than the stall bound |
| selfish | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| selfish | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| reorg-60 | reorg 60 < 120: honest nodes reorganize to the heavier chain | pass | deepest honest reorg 48; 7/7 on the attacker's tip |
| reorg-60 | safety: converge after quiesce | pass | 8 honest nodes at height 351 (8b339e4c) |
| reorg-60 | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 3499 block applications on 9 nodes |
| reorg-60 | no node panicked | pass | none |
| reorg-60 | resource: per-block apply time ≤ 2 s | pass | p99 6 ms, max 47 ms over 3506 applications; template max 0 ms |
| reorg-60 | no honest peer banned | pass | none |
| reorg-60 | resource: mempool upkeep per tip move ≤ 2 s | pass | max 1 ms (remove mined + re-admit the pool) |
| reorg-60 | resource: peak RSS | info | 69 MB (process) |
| reorg-60 | resource: mempool ≤ capacity | pass | max 0 txs |
| reorg-60 | liveness: no stall | pass | no honest node waited longer than the stall bound |
| reorg-60 | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| reorg-60 | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| reorg-140 | reorg 140 ≥ 120: refused by every honest node (chain.md §5.2) | pass | deepest 1; 24 deep-fork refusals; 0/7 on the attacker's tip |
| reorg-140 | late joiner after a refused deep rewrite (checkpoints off) | info | late joiner at 490 follows the attacker's chain; checkpoints off |
| reorg-140 | safety: converge after quiesce | pass | 7 honest nodes at height 481 (d00981ca) |
| reorg-140 | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 4400 block applications on 9 nodes |
| reorg-140 | no node panicked | pass | none |
| reorg-140 | resource: per-block apply time ≤ 2 s | pass | p99 6 ms, max 33 ms over 4561 applications; template max 0 ms |
| reorg-140 | no honest peer banned | pass | none |
| reorg-140 | resource: mempool upkeep per tip move ≤ 2 s | pass | max 1 ms (remove mined + re-admit the pool) |
| reorg-140 | resource: peak RSS | info | 74 MB (process) |
| reorg-140 | resource: mempool ≤ capacity | pass | max 0 txs |
| reorg-140 | liveness: no stall | pass | no honest node waited longer than the stall bound |
| reorg-140 | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| reorg-140 | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| eclipse | eclipse: victim recovers when the fake branch is < 120 deep | pass | eclipse 30 min by a 5 % miner: attacker chain grew 56 blocks; victim at 209 (recovered), honest at 209 |
| eclipse | safety: converge after quiesce | pass | 7 honest nodes at height 212 (999cda64) |
| eclipse | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 1621 block applications on 8 nodes |
| eclipse | no node panicked | pass | none |
| eclipse | resource: per-block apply time ≤ 2 s | pass | p99 5 ms, max 14 ms over 1621 applications; template max 0 ms |
| eclipse | no honest peer banned | pass | none |
| eclipse | resource: mempool upkeep per tip move ≤ 2 s | pass | max 0 ms (remove mined + re-admit the pool) |
| eclipse | resource: peak RSS | info | 74 MB (process) |
| eclipse | resource: mempool ≤ capacity | pass | max 0 txs |
| eclipse | liveness: no stall | pass | no honest node waited longer than the stall bound |
| eclipse | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| eclipse | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| flood | flood: block fullness | info | largest body 224 KB of 256 KB; 95 blocks |
| flood | safety: converge after quiesce | pass | 5 honest nodes at height 97 (c4fb2af9) |
| flood | honest txs during the flood (fee +100): honest txs mined | pass | 511/511 mined (100 %), wait p50 27 s, p90 99 s, max 177 s; 0 refused at submit (bound ≥ 90 %) |
| flood | fee floor: below-floor txs refused | pass | 0 below-floor admitted; 30424 flood txs admitted, 29576 refused |
| flood | mempool within capacity | pass | max 8192 txs, 1792 KB across nodes (cap 8192 txs) |
| flood | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 485 block applications on 5 nodes |
| flood | no node panicked | pass | none |
| flood | resource: per-block apply time ≤ 2 s | pass | p99 47 ms, max 49 ms over 485 applications; template max 48 ms |
| flood | no honest peer banned | pass | none |
| flood | resource: mempool upkeep per tip move ≤ 2 s | pass | max 145 ms (remove mined + re-admit the pool) |
| flood | resource: peak RSS | info | 261 MB (process) |
| flood | resource: mempool ≤ capacity | pass | max 8192 txs |
| flood | liveness: no stall | pass | no honest node waited longer than the stall bound |
| flood | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| flood | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| hostile | legacy claim credited exactly once | pass | claimant holds 4999700 of 5000000 (fee 300); double claims, wrong-key claims refused |
| hostile | safety: converge after quiesce | pass | 6 honest nodes at height 243 (87326e44) |
| hostile | traffic: honest txs mined | pass | 1417/1417 mined (100 %), wait p50 20 s, p90 73 s, max 178 s; 0 refused at submit (bound ≥ 90 %) |
| hostile | hostile RandomBytes: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| hostile | hostile OversizeGossip: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| hostile | hostile BadSignature: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| hostile | hostile OversizeTx: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| hostile | hostile Replay: refused, not scored | pass | 12 messages sent; banned by 0/3 targets |
| hostile | hostile CrossChain: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| hostile | hostile ClaimWrongKey: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| hostile | hostile DoubleClaim: refused, not scored | pass | 12 messages sent; banned by 0/3 targets |
| hostile | hostile BodyOnGossip: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| hostile | hostile BadPowHeader: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| hostile | hostile WrongChainStatus: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| hostile | forged block Oversize: never on an honest main chain | pass | height 44; on 0/6 honest main chains |
| hostile | forged block TxRoot: never on an honest main chain | pass | height 47; on 0/6 honest main chains |
| hostile | forged block StateRoot: never on an honest main chain | pass | height 53; on 0/6 honest main chains |
| hostile | forged block CoinbaseAmount: never on an honest main chain | pass | height 55; on 0/6 honest main chains |
| hostile | forged block BadSigTx: never on an honest main chain | pass | height 57; on 0/6 honest main chains |
| hostile | forged block DuplicateTx: never on an honest main chain | pass | height 61; on 0/6 honest main chains |
| hostile | forged block TooManyTxs: never on an honest main chain | pass | height 65; on 0/6 honest main chains |
| hostile | forged block FutureTime: never on an honest main chain | pass | height 72; on 0/6 honest main chains |
| hostile | forged block WrongBits: never on an honest main chain | pass | height 77; on 0/6 honest main chains |
| hostile | forged blocks: no honest relay banned | pass | no honest peer banned |
| hostile | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 2580 block applications on 18 nodes |
| hostile | no node panicked | pass | none |
| hostile | resource: per-block apply time ≤ 2 s | pass | p99 9 ms, max 16 ms over 2613 applications; template max 4 ms |
| hostile | no honest peer banned | pass | none |
| hostile | resource: mempool upkeep per tip move ≤ 2 s | pass | max 0 ms (remove mined + re-admit the pool) |
| hostile | resource: peak RSS | info | 264 MB (process) |
| hostile | resource: mempool ≤ capacity | pass | max 40 txs |
| hostile | liveness: no stall | pass | no honest node waited longer than the stall bound |
| hostile | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| hostile | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| economy | safety: converge after quiesce | pass | 6 honest nodes at height 353 (b92c6291) |
| economy | traffic: honest txs mined | pass | 1305/1342 mined (97 %), wait p50 23 s, p90 98 s, max 262 s; 0 refused at submit (bound ≥ 90 %) |
| economy | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 2123 block applications on 6 nodes |
| economy | no node panicked | pass | none |
| economy | resource: per-block apply time ≤ 2 s | pass | p99 9 ms, max 26 ms over 2124 applications; template max 4 ms |
| economy | no honest peer banned | pass | none |
| economy | resource: mempool upkeep per tip move ≤ 2 s | pass | max 0 ms (remove mined + re-admit the pool) |
| economy | resource: peak RSS | info | 264 MB (process) |
| economy | resource: mempool ≤ capacity | pass | max 37 txs |
| economy | liveness: no stall | pass | no honest node waited longer than the stall bound |
| economy | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| economy | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| economy | economy: settlements happened | pass | 8 epochs settled, 1920982 paid in total |
| economy | determinism: epoch records equal on every node | pass | 6 nodes × 8 epochs |
| economy | economy: P ≤ B = ⌊F/4⌋, Σ payouts = P | pass | 8 records |
| economy | economy: pay(c) ≤ 1 % of Δs(c) | pass | 19 payouts within cap |
| economy | economy §9: recycler earns ≤ the mark's decay | expected red | first lock paid 43704, relock paid 1093585 (by epoch {2: 43704, 5: 1093585}; relock the only earner in epochs [5]) |
| economy | economy §9: hedge (+1/−1 equal) earns nothing | expected red | +1 side paid 71569, −1 side paid 0 |
| economy | economy §9: Sybil splitting changes no total | pass | one key 149412, split over 8 keys 149408 |
| economy | economy §9: self-linking ring ≤ 1 % of its stake per epoch | pass | ring of 4 positions, stake 20000000000, paid 0 over 8 epochs |
| economy | economy §9: last-block copy earns ≤ the original | pass | original 17900, late copy 17900 |
| economy | economy §3.5: paid to the payout owner of G_E | pass | payout lock epoch 2, owner changed after its boundary; paid [(2, "author")] |
| economy | economy §9: −1 griefer payoff | info | paid 0 (earns only if zeroing raised syntropy) |
| economy | economy: concentration vs diverse honest stakers | info | concentrator paid 285656; honest diverse stakers paid 27966 |
| settle-slow | slow settlement: the chain waits at boundary + D, then resumes | pass | settlement compute 25 min vs D = 20 blocks; 15 blocks deferred; longest tip gap 24 min; height 379 |
| settle-slow | safety: converge after quiesce | pass | 6 honest nodes at height 405 (010e82bc) |
| settle-slow | traffic: honest txs mined | pass | 2374/2374 mined (100 %), wait p50 464 s, p90 1200 s, max 1471 s; 0 refused at submit (bound ≥ 90 %) |
| settle-slow | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 2563 block applications on 6 nodes |
| settle-slow | no node panicked | pass | none |
| settle-slow | resource: per-block apply time ≤ 2 s | pass | p99 15 ms, max 17 ms over 2798 applications; template max 15 ms |
| settle-slow | no honest peer banned | pass | none |
| settle-slow | resource: mempool upkeep per tip move ≤ 2 s | pass | max 1 ms (remove mined + re-admit the pool) |
| settle-slow | resource: peak RSS | info | 264 MB (process) |
| settle-slow | resource: mempool ≤ capacity | pass | max 263 txs |
| settle-slow | liveness: no stall | pass | no honest node waited longer than the stall bound |
| settle-slow | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| settle-slow | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| settle-panic | settlement engine panics: node behaviour | info | 4/4 nodes crashed at settle(1) = height 60 (highest applied 60); a restart reopens below settle(1) and crashes again on the next block — a deterministic engine failure halts the chain (chain-economy §2.5: never guess) |

### nightly

| scenario | invariant | verdict | evidence |
|---|---|---|---|
| steady-large | liveness: mean block interval 30 s ± 20 % | pass | 935 blocks, mean 30.2 s, p99 130 s, max 173 s (bound 24–36 s) |
| steady-large | stale blocks ≤ 5 % | pass | 10 of 968 found blocks off the main chain |
| steady-large | safety: converge after quiesce | pass | 24 honest nodes at height 960 (688e48b3) |
| steady-large | honest traffic: honest txs mined | pass | 7115/7115 mined (100 %), wait p50 20 s, p90 67 s, max 170 s; 0 refused at submit (bound ≥ 90 %) |
| steady-large | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 23091 block applications on 24 nodes |
| steady-large | no node panicked | pass | none |
| steady-large | resource: per-block apply time ≤ 2 s | pass | p99 9 ms, max 28 ms over 23192 applications; template max 3 ms |
| steady-large | no honest peer banned | pass | none |
| steady-large | resource: mempool upkeep per tip move ≤ 2 s | pass | max 49 ms (remove mined + re-admit the pool) |
| steady-large | resource: peak RSS | info | 348 MB (process) |
| steady-large | resource: mempool ≤ capacity | pass | max 51 txs |
| steady-large | liveness: no stall | pass | no honest node waited longer than the stall bound |
| steady-large | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| steady-large | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| steady-large | checkpoints reach every node | pass | 7 checkpoints held by node 1 |
| partition-long | partition > 1 h: minority refuses the deep reorg (chain.md §5.2) | info | 3 deep-fork refusals on minority nodes before the operator resync |
| partition-long | safety: converge after quiesce | pass | 10 honest nodes at height 414 (e9b3ab2c) |
| partition-long | traffic: honest txs mined | pass | 1549/2114 mined (73 %), wait p50 20 s, p90 65 s, max 166 s; 44 refused at submit (bound ≥ 60 %) |
| partition-long | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 5251 block applications on 10 nodes |
| partition-long | no node panicked | pass | none |
| partition-long | resource: per-block apply time ≤ 2 s | pass | p99 10 ms, max 124 ms over 5281 applications; template max 15 ms |
| partition-long | no honest peer banned | pass | none |
| partition-long | resource: mempool upkeep per tip move ≤ 2 s | pass | max 6 ms (remove mined + re-admit the pool) |
| partition-long | resource: peak RSS | info | 350 MB (process) |
| partition-long | resource: mempool ≤ capacity | pass | max 61 txs |
| partition-long | liveness: no stall | pass | no honest node waited longer than the stall bound |
| partition-long | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| partition-long | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| asert-100x | ASERT ×100 spike: interval back to 30 s ± 20 % | pass | 1246 s after the step |
| asert-100x | ASERT ÷100 collapse: interval back to 30 s ± 20 % | pass | 8613 s after the step (window 19200 s) |
| asert-100x | ASERT ÷100: first block after the collapse, longest gap | info | first block 6929 s after the step (exponential, mean 3000 s: P(> 1 h) = e^(−3600/3000)), longest gap 6957 s |
| asert-100x | liveness: steady again after the steps | pass | 247 blocks, mean 28.9 s, p99 104 s, max 133 s (bound 22–38 s) |
| asert-100x | safety: converge after quiesce | pass | 6 honest nodes at height 1425 (aecbfe6a) |
| asert-100x | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 8550 block applications on 6 nodes |
| asert-100x | no node panicked | pass | none |
| asert-100x | resource: per-block apply time ≤ 2 s | pass | p99 5 ms, max 41 ms over 8629 applications; template max 1 ms |
| asert-100x | no honest peer banned | pass | none |
| asert-100x | resource: mempool upkeep per tip move ≤ 2 s | pass | max 3 ms (remove mined + re-admit the pool) |
| asert-100x | resource: peak RSS | info | 351 MB (process) |
| asert-100x | resource: mempool ≤ capacity | pass | max 0 txs |
| asert-100x | liveness: no stall | pass | no honest node waited longer than the stall bound |
| asert-100x | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| asert-100x | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| selfish-40 | selfish mining at α = 40 %: revenue share | info | 43.5 % of 950 main-chain blocks (hashrate 40.0 %); deepest honest reorg 11 |
| selfish-40 | liveness under selfish mining | pass | 933 blocks, mean 30.2 s, p99 156 s, max 296 s (bound 22–40 s) |
| selfish-40 | selfish: honest reorgs stay shallow | pass | deepest 11 |
| selfish-40 | safety: converge after quiesce | pass | 8 honest nodes at height 952 (6746ab2a) |
| selfish-40 | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 10127 block applications on 9 nodes |
| selfish-40 | no node panicked | pass | none |
| selfish-40 | resource: per-block apply time ≤ 2 s | pass | p99 7 ms, max 26 ms over 10421 applications; template max 1 ms |
| selfish-40 | no honest peer banned | pass | none |
| selfish-40 | resource: mempool upkeep per tip move ≤ 2 s | pass | max 2 ms (remove mined + re-admit the pool) |
| selfish-40 | resource: peak RSS | info | 352 MB (process) |
| selfish-40 | resource: mempool ≤ capacity | pass | max 0 txs |
| selfish-40 | liveness: no stall | pass | no honest node waited longer than the stall bound |
| selfish-40 | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| selfish-40 | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| reorg-140-checkpoints | reorg 140 ≥ 120: refused by every honest node (chain.md §5.2) | pass | deepest 2; 19 deep-fork refusals; 0/7 on the attacker's tip |
| reorg-140-checkpoints | late joiner rejects a deep rewrite (checkpoints on) | pass | late joiner at 473 follows the honest chain; checkpoints on |
| reorg-140-checkpoints | safety: converge after quiesce | pass | 7 honest nodes at height 475 (718bd771) |
| reorg-140-checkpoints | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 4293 block applications on 9 nodes |
| reorg-140-checkpoints | no node panicked | pass | none |
| reorg-140-checkpoints | resource: per-block apply time ≤ 2 s | pass | p99 7 ms, max 88 ms over 4326 applications; template max 0 ms |
| reorg-140-checkpoints | no honest peer banned | pass | none |
| reorg-140-checkpoints | resource: mempool upkeep per tip move ≤ 2 s | pass | max 1 ms (remove mined + re-admit the pool) |
| reorg-140-checkpoints | resource: peak RSS | info | 352 MB (process) |
| reorg-140-checkpoints | resource: mempool ≤ capacity | pass | max 0 txs |
| reorg-140-checkpoints | liveness: no stall | pass | no honest node waited longer than the stall bound |
| reorg-140-checkpoints | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| reorg-140-checkpoints | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| reorg-140-checkpoints-unfetched | reorg 140 ≥ 120: refused by every honest node (chain.md §5.2) | pass | deepest 2; 26 deep-fork refusals; 0/7 on the attacker's tip |
| reorg-140-checkpoints-unfetched | late joiner rejects a deep rewrite (checkpoints never fetched) | expected red | late joiner at 490 follows the attacker's chain; checkpoints on |
| reorg-140-checkpoints-unfetched | safety: converge after quiesce | pass | 7 honest nodes at height 482 (8d562ad1) |
| reorg-140-checkpoints-unfetched | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 4416 block applications on 9 nodes |
| reorg-140-checkpoints-unfetched | no node panicked | pass | none |
| reorg-140-checkpoints-unfetched | resource: per-block apply time ≤ 2 s | pass | p99 6 ms, max 35 ms over 4586 applications; template max 0 ms |
| reorg-140-checkpoints-unfetched | no honest peer banned | pass | none |
| reorg-140-checkpoints-unfetched | resource: mempool upkeep per tip move ≤ 2 s | pass | max 1 ms (remove mined + re-admit the pool) |
| reorg-140-checkpoints-unfetched | resource: peak RSS | info | 354 MB (process) |
| reorg-140-checkpoints-unfetched | resource: mempool ≤ capacity | pass | max 0 txs |
| reorg-140-checkpoints-unfetched | liveness: no stall | pass | no honest node waited longer than the stall bound |
| reorg-140-checkpoints-unfetched | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| reorg-140-checkpoints-unfetched | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| eclipse-long | eclipse > 1 h: the victim rejoins without an operator | expected red | eclipse 150 min by a 5 % miner: attacker chain grew 292 blocks; victim at 239 (stuck on the attacker's branch), honest at 460 |
| eclipse-long | safety: converge after quiesce | pass | 6 honest nodes at height 462 (7cf9ac79) |
| eclipse-long | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 3446 block applications on 8 nodes |
| eclipse-long | no node panicked | pass | none |
| eclipse-long | resource: per-block apply time ≤ 2 s | pass | p99 5 ms, max 10 ms over 3455 applications; template max 0 ms |
| eclipse-long | no honest peer banned | pass | none |
| eclipse-long | resource: mempool upkeep per tip move ≤ 2 s | pass | max 0 ms (remove mined + re-admit the pool) |
| eclipse-long | resource: peak RSS | info | 354 MB (process) |
| eclipse-long | resource: mempool ≤ capacity | pass | max 0 txs |
| eclipse-long | liveness: no stall | pass | no honest node waited longer than the stall bound |
| eclipse-long | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| eclipse-long | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| forged-header-storm | legacy claim credited exactly once | pass | claimant holds 4999700 of 5000000 (fee 300); double claims, wrong-key claims refused |
| forged-header-storm | safety: converge after quiesce | pass | 6 honest nodes at height 598 (9b76574c) |
| forged-header-storm | traffic: honest txs mined | pass | 1417/1417 mined (100 %), wait p50 20 s, p90 72 s, max 217 s; 0 refused at submit (bound ≥ 90 %) |
| forged-header-storm | hostile RandomBytes: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| forged-header-storm | hostile OversizeGossip: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| forged-header-storm | hostile BadSignature: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| forged-header-storm | hostile OversizeTx: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| forged-header-storm | hostile Replay: refused, not scored | pass | 12 messages sent; banned by 0/3 targets |
| forged-header-storm | hostile CrossChain: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| forged-header-storm | hostile ClaimWrongKey: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| forged-header-storm | hostile DoubleClaim: refused, not scored | pass | 12 messages sent; banned by 0/3 targets |
| forged-header-storm | hostile BodyOnGossip: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| forged-header-storm | hostile BadPowHeader: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| forged-header-storm | hostile WrongChainStatus: banned by every target | pass | 12 messages sent; banned by 3/3 targets |
| forged-header-storm | forged block Oversize: never on an honest main chain | pass | height 44; on 0/6 honest main chains |
| forged-header-storm | forged block TxRoot: never on an honest main chain | pass | height 47; on 0/6 honest main chains |
| forged-header-storm | forged block StateRoot: never on an honest main chain | pass | height 53; on 0/6 honest main chains |
| forged-header-storm | forged block CoinbaseAmount: never on an honest main chain | pass | height 55; on 0/6 honest main chains |
| forged-header-storm | forged block BadSigTx: never on an honest main chain | pass | height 57; on 0/6 honest main chains |
| forged-header-storm | forged block DuplicateTx: never on an honest main chain | pass | height 61; on 0/6 honest main chains |
| forged-header-storm | forged block TooManyTxs: never on an honest main chain | pass | height 65; on 0/6 honest main chains |
| forged-header-storm | forged block FutureTime: never on an honest main chain | pass | height 72; on 0/6 honest main chains |
| forged-header-storm | forged block WrongBits: never on an honest main chain | pass | height 77; on 0/6 honest main chains |
| forged-header-storm | forged blocks: no honest relay banned | pass | no honest peer banned |
| forged-header-storm | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 5867 block applications on 18 nodes |
| forged-header-storm | no node panicked | pass | none |
| forged-header-storm | resource: per-block apply time ≤ 2 s | pass | p99 8 ms, max 22 ms over 6042 applications; template max 3 ms |
| forged-header-storm | no honest peer banned | pass | none |
| forged-header-storm | resource: mempool upkeep per tip move ≤ 2 s | pass | max 2 ms (remove mined + re-admit the pool) |
| forged-header-storm | resource: peak RSS | info | 355 MB (process) |
| forged-header-storm | resource: mempool ≤ capacity | pass | max 47 txs |
| forged-header-storm | liveness: no stall | pass | no honest node waited longer than the stall bound |
| forged-header-storm | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| forged-header-storm | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| flood-long | flood: block fullness | info | largest body 224 KB of 256 KB; 316 blocks |
| flood-long | safety: converge after quiesce | pass | 5 honest nodes at height 318 (4513e6c9) |
| flood-long | honest txs during the flood (fee +100): honest txs mined | pass | 1747/1747 mined (100 %), wait p50 20 s, p90 76 s, max 195 s; 0 refused at submit (bound ≥ 90 %) |
| flood-long | fee floor: below-floor txs refused | pass | 0 below-floor admitted; 283066 flood txs admitted, 436934 refused |
| flood-long | mempool within capacity | pass | max 8192 txs, 1792 KB across nodes (cap 8192 txs) |
| flood-long | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 1595 block applications on 5 nodes |
| flood-long | no node panicked | pass | none |
| flood-long | resource: per-block apply time ≤ 2 s | pass | p99 64 ms, max 116 ms over 1602 applications; template max 49 ms |
| flood-long | no honest peer banned | pass | none |
| flood-long | resource: mempool upkeep per tip move ≤ 2 s | pass | max 320 ms (remove mined + re-admit the pool) |
| flood-long | resource: peak RSS | info | 947 MB (process) |
| flood-long | resource: mempool ≤ capacity | pass | max 8192 txs |
| flood-long | liveness: no stall | pass | no honest node waited longer than the stall bound |
| flood-long | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| flood-long | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| economy-long | safety: converge after quiesce | pass | 6 honest nodes at height 987 (caed5296) |
| economy-long | traffic: honest txs mined | pass | 3694/3728 mined (99 %), wait p50 21 s, p90 81 s, max 262 s; 0 refused at submit (bound ≥ 90 %) |
| economy-long | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 5936 block applications on 6 nodes |
| economy-long | no node panicked | pass | none |
| economy-long | resource: per-block apply time ≤ 2 s | pass | p99 8 ms, max 12 ms over 5955 applications; template max 6 ms |
| economy-long | no honest peer banned | pass | none |
| economy-long | resource: mempool upkeep per tip move ≤ 2 s | pass | max 0 ms (remove mined + re-admit the pool) |
| economy-long | resource: peak RSS | info | 947 MB (process) |
| economy-long | resource: mempool ≤ capacity | pass | max 35 txs |
| economy-long | liveness: no stall | pass | no honest node waited longer than the stall bound |
| economy-long | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| economy-long | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |
| economy-long | economy: settlements happened | pass | 24 epochs settled, 1920982 paid in total |
| economy-long | determinism: epoch records equal on every node | pass | 6 nodes × 24 epochs |
| economy-long | economy: P ≤ B = ⌊F/4⌋, Σ payouts = P | pass | 24 records |
| economy-long | economy: pay(c) ≤ 1 % of Δs(c) | pass | 19 payouts within cap |
| economy-long | economy §9: recycler earns ≤ the mark's decay | expected red | first lock paid 43704, relock paid 1093585 (by epoch {2: 43704, 5: 1093585}; relock the only earner in epochs [5]) |
| economy-long | economy §9: hedge (+1/−1 equal) earns nothing | expected red | +1 side paid 71569, −1 side paid 0 |
| economy-long | economy §9: Sybil splitting changes no total | pass | one key 149412, split over 8 keys 149408 |
| economy-long | economy §9: self-linking ring ≤ 1 % of its stake per epoch | pass | ring of 4 positions, stake 20000000000, paid 0 over 24 epochs |
| economy-long | economy §9: last-block copy earns ≤ the original | pass | original 17900, late copy 17900 |
| economy-long | economy §3.5: paid to the payout owner of G_E | pass | payout lock epoch 2, owner changed after its boundary; paid [(2, "author")] |
| economy-long | economy §9: −1 griefer payoff | info | paid 0 (earns only if zeroing raised syntropy) |
| economy-long | economy: concentration vs diverse honest stakers | info | concentrator paid 285656; honest diverse stakers paid 27966 |
| settle-slow-one | slow settlement: the chain waits at boundary + D, then resumes | pass | settlement compute 25 min vs D = 20 blocks; 39463 blocks deferred; longest tip gap 23 min; height 473 |
| settle-slow-one | safety: converge after quiesce | pass | 6 honest nodes at height 477 (3bcf6280) |
| settle-slow-one | traffic: honest txs mined | pass | 2374/2374 mined (100 %), wait p50 20 s, p90 75 s, max 1293 s; 0 refused at submit (bound ≥ 90 %) |
| settle-slow-one | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 2890 block applications on 6 nodes |
| settle-slow-one | no node panicked | pass | none |
| settle-slow-one | resource: per-block apply time ≤ 2 s | pass | p99 5 ms, max 30 ms over 48799 applications; template max 13 ms |
| settle-slow-one | no honest peer banned | pass | none |
| settle-slow-one | resource: mempool upkeep per tip move ≤ 2 s | pass | max 1 ms (remove mined + re-admit the pool) |
| settle-slow-one | resource: peak RSS | info | 1354 MB (process) |
| settle-slow-one | resource: mempool ≤ capacity | pass | max 232 txs |
| settle-slow-one | liveness: no stall | pass | no honest node waited longer than the stall bound |
| settle-slow-one | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| settle-slow-one | safety: reorg depth ≤ 119 | pass | no reorganization deeper than MAX_REORG |

### launch

| scenario | invariant | verdict | evidence |
|---|---|---|---|
| launch-pussy | launch: chain id = the pinned genesis particle | pass | bae85f40d9f50866 (pinned bae85f40d9f50866) |
| launch-pussy | launch: block 0 and state root = the pinned values | pass | block 0 73208ee2d43fbb04 root d48fee478584d248; opened 3 nodes in [595, 588, 579] ms; RSS 152 MB |
| launch-pussy | launch: the chain reaches settle(2) + 10 | pass | height 620 at virtual 310 min (10753 M hashes ground on metal (Apple M4 Max)) |
| launch-pussy | launch: first block at genesis difficulty (2^232) | info | found at virtual 19 s with 100 kH/s mining (expected ≈ 168 s) |
| launch-pussy | launch: mean interval 30 s ± 25 % once miners joined | pass | 500 blocks after 60 min, mean 29.9 s, p99 132 s, max 186 s |
| launch-pussy | launch: late joiner syncs from genesis through two settlements | pass | started at virtual 200 min (network at 402), opened in 616 ms, now at 620 of 620 |
| launch-pussy | launch: settlement compute ≤ SETTLE_BUDGET (900 s) | pass | max 723 ms; n0:e2 722 ms (26805 axons), n0:e1 608 ms (26805 axons), n1:e2 713 ms (26805 axons), n1:e1 591 ms (26805 axons), n2:e2 702 ms (26805 axons), n2:e1 613 ms (26805 axons), n3:e2 723 ms (26805 axons), n3:e1 372 ms (26805 axons) |
| launch-pussy | launch: record syntropy fits i64 (not saturated) | pass | 2 records, syntropy [4156823547489567535, 4156823547489567535] |
| launch-pussy | economy: settlements happened | pass | 2 epochs settled, 62750550 paid in total |
| launch-pussy | determinism: epoch records equal on every node | pass | 4 nodes × 2 epochs |
| launch-pussy | economy: P ≤ B = ⌊F/4⌋, Σ payouts = P | pass | 2 records |
| launch-pussy | economy: pay(c) ≤ 1 % of Δs(c) | pass | 1 payouts within cap |
| launch-pussy | economy: concentration vs diverse honest stakers | info | concentrator paid 0; honest diverse stakers paid 0 |
| launch-pussy | safety: converge after quiesce | pass | 4 honest nodes at height 622 (9bd75e6d) |
| launch-pussy | traffic from matured coinbases: honest txs mined | pass | 711/733 mined (96 %), wait p50 22 s, p90 76 s, max 161 s; 6 refused at submit (bound ≥ 90 %) |
| launch-pussy | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 2494 applications |
| launch-pussy | resource: per-block apply time at the tip ≤ 2 s (settlement blocks included) | pass | max 11 ms on the nodes that followed the tip |
| launch-pussy | resource: catching up computes every settlement inline | info | late joiner: max 311 ms on one block (a settle(E) block); a node syncing history pays up to 0 s of settlement per epoch (12 epochs a day) |
| launch-pussy | resource: peak RSS | info | 340 MB for 4 nodes |
| launch-pussy | liveness: no stall | pass | no honest node waited longer than 40 min |
| launch-pussy | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
| launch-bostrom | launch: chain id = the pinned genesis particle | pass | f78c60edd6aea30a (pinned f78c60edd6aea30a) |
| launch-bostrom | launch: block 0 and state root = the pinned values | pass | block 0 a13f0f627dae47a0 root fa0c2c9404a6eff0; opened 2 nodes in [59166, 58252] ms; RSS 11414 MB |
| launch-bostrom | launch: the chain reaches settle(2) + 10 | pass | height 623 at virtual 310 min (7383 M hashes ground on metal (Apple M4 Max)) |
| launch-bostrom | launch: first block at genesis difficulty (2^232) | info | found at virtual 23 s with 100 kH/s mining (expected ≈ 168 s) |
| launch-bostrom | launch: mean interval 30 s ± 25 % once miners joined | pass | 522 blocks after 60 min, mean 28.5 s, p99 136 s, max 150 s |
| launch-bostrom | launch: late joiner syncs from genesis through two settlements | pass | started at virtual 210 min (network at 396), opened in 59406 ms, now at 623 of 623 |
| launch-bostrom | launch: settlement compute ≤ SETTLE_BUDGET (900 s) | pass | max 52003 ms; n0:e2 52003 ms (2990817 axons), n0:e1 44647 ms (2990817 axons), n1:e2 51090 ms (2990817 axons), n1:e1 44925 ms (2990817 axons), n2:e2 50383 ms (2990817 axons), n2:e1 37165 ms (2990817 axons) |
| launch-bostrom | launch: record syntropy fits i64 (not saturated) | expected red | epoch 1 syntropy 9223372036854775807; epoch 2 syntropy 9223372036854775807 |
| launch-bostrom | economy: settlements happened | pass | 2 epochs settled, 0 paid in total |
| launch-bostrom | determinism: epoch records equal on every node | pass | 3 nodes × 2 epochs |
| launch-bostrom | economy: P ≤ B = ⌊F/4⌋, Σ payouts = P | pass | 2 records |
| launch-bostrom | economy: pay(c) ≤ 1 % of Δs(c) | pass | 0 payouts within cap |
| launch-bostrom | economy: concentration vs diverse honest stakers | info | concentrator paid 0; honest diverse stakers paid 0 |
| launch-bostrom | safety: converge after quiesce | pass | 3 honest nodes at height 625 (df7c478f) |
| launch-bostrom | traffic from matured coinbases: honest txs mined | pass | 179/183 mined (97 %), wait p50 22 s, p90 71 s, max 120 s; 547 refused at submit (bound ≥ 90 %) |
| launch-bostrom | conservation every block (Σ native = S0 + h·SUBSIDY) | pass | 1881 applications |
| launch-bostrom | resource: per-block apply time at the tip ≤ 2 s (settlement blocks included) | pass | max 11 ms on the nodes that followed the tip |
| launch-bostrom | resource: catching up computes every settlement inline | info | late joiner: max 37248 ms on one block (a settle(E) block); a node syncing history pays up to 52 s of settlement per epoch (12 epochs a day) |
| launch-bostrom | resource: peak RSS | info | 20476 MB for 3 nodes |
| launch-bostrom | liveness: no stall | pass | no honest node waited longer than 40 min |
| launch-bostrom | safety: agree below finality | pass | honest nodes agreed below depth 120 at every sample |
