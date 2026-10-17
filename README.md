# simulation

two simulators: the closed-loop emergence simulator (`simulation
emergence …`, the historical `simulation --flags`, everything up to §
doctrine) and the network + economy simulator of the cyber phase-1 chain
(`simulation chain …`, § chain).

closed-loop emergence simulator. tests the hypothesis recorded in
[[crystal/emergence.md]]: **emergence requires a closed feedback loop,
not just scale**. the tru scale ladder (tru/eval/scale_ladder.md)
confirmed the negative half — open-loop gradient training does not
become generative from 87k to 2.8M particles. this repo tests the
positive half: when a fraction `g` of linking agents follows the
compiled model's suggestions, does the graph start encoding
model-accessible regularities — and at what coupling does unseen-link
predictivity run away?

one graph grows; a compiled embedding is recomputed on a cadence; a
fraction of link events uses the model's top suggestion instead of the
hidden rule. metrics stream as JSONL.

## why performance is a first-class requirement

the parameter space is a sweep grid: coupling g x size n x growth-rule
q x cadence x d x seed. hundreds of runs, each touching every edge and
every node of its graph many times. scalar code that is "fast enough
for one run" is a wall for the grid. so the hot kernels sit behind a
trait from day one, with a scalar implementation now and a honeycrisp
backend as the documented next step (see below).

## the loop

```
                 +-------------------+
                 |   hidden rule     |  PA + triadic closure (the
                 |  (physics model   |  "physics" the model cannot
                 |    cannot see)    |  see — kept as counterfactual)
                 +---------+---------+
                           | (1-g) of events
 link event                v
 (source s active)   +-------------+      compile cadence
        |            |             |<------------------+
        | (g)        |  growing    |                   |
        v            |  sparse     |            +------+------+
 +-------------+     |  graph      |            | spectrum of |
 | model score |---->|             |            | D(A+.5A^2)D |
 | cos(E_s,E_v)|     +------+------+            | E = U sqrt  |
 +-------------+            |                   +------+------+
        ^                   | metrics                  |
        |                   v                          v
 unseen-MRR on reserved    JSONL                suggestions
 future links (probe)                             (top-1)
```

## usage

```bash
cargo build --release
./target/release/simulation \
  --nodes 1000000 --g 0.5 --q 0.5 --d 64 --compile-every 200000 \
  --probe 500 --seed 7 --out runs/g05_n1M.jsonl
```

flags: `--nodes` target size · `--g` loop coupling [0,1] · `--q`
triadic-closure probability · `--d` embedding width ·
`--compile-every` events between compiles · `--probe` held-out future
links for the unseen metric · `--seen-probe` train-edge control ·
`--seed`. JSONL metrics: one line per compile tick — unseen/seen MRR,
edge count, λ-style spectral stats, wall time per phase.

## honeycrisp backend (this repo USES honeycrisp)

`--kernels neon` routes the profiled-hot `dot` through
`honeycrisp/acpu`'s NEON kernel (this repo depends on acpu by path;
everything else falls back to scalar until its op lands). measured on
M-series: dot 64K 38.2 us -> 4.7 us (**8.16x**); a full 60k-node
compile tick 6.35 s -> 3.13 s; whole sweep-1 run 135 s -> 98 s.
`cargo run --release --example bench_kernels` re-measures. next ops in
order: axpy (ortho inner loop), then the k x k Gram on AMX.

## honeycrisp performance map

hot kernels, in the order they will bind (measured, not guessed —
`cargo run --release --example bench_kernels` prints op/s):

| kernel | where | shape | honeycrisp path |
|--------|-------|-------|-----------------|
| `gemv` | suggestions, probes | scores = E @ x, E is n×d f32 | NEON f32x8 dot now; batch it into a skinny SGEMM for AMX/Metal (aruminium) when suggestion batches grow |
| `ortho` | compile, every iteration | Gram + normalize over k×n block | the pussy wall was scalar Gram-Schmidt at 1.27 s/iter — NEON axpy/dot gives ~4×, AMX for the k×k Gram |
| `spmv` | compile matvec | A·x, A²·x over CSR | memory-bound; unimem arena for edge-array locality, no compute unit helps latency — profile first |
| `topk` | suggestion select | argmax over n scores | NEON reductions; trivial next to gemv |
| growth | event loop | pointer-chasing, RNG | none — keep branch-light, arena-allocated |

data layout commitments made NOW so the swap is drop-in: E and all
kernel buffers are row-major f32; the kernel trait is the only caller
of raw loops.

measured on this machine (M-series, scalar f32, `bench_kernels`):
gemv n=1M d=64 9.9 ms (26 GB/s, memory-bound); spmv n=1M deg=3 1.7 ms
(7 GB/s, latency-bound — the NEON win is in ortho, not here); compile
of a 20k-node graph 1.9 s (iters=25, k=64). compile cost scales
~linearly in n — that is the rung-size governor until the ortho lands
on NEON.

the unseen metric is a FORECAST: at the probe window all reserved
future links are ranked by the freshly compiled model (they are not in
the graph); the curve then decays as the reserved edges materialize.

## doctrine

- black box: the simulator never reads node contents; the hidden rule
  is the only ground truth and stays unobserved.
- deterministic: seeded xorshift; same flags -> same JSONL.
- honest metrics: probes are reserved BEFORE the run starts; unseen
  links never enter training or compilation.
- ASCII-only, no external crates (the emergence simulator; `chain/` depends on soft3).


## chain — the network + economy simulator of the phase-1 chain

`simulation chain <gate|nightly|launch|scenario…>` (or the standalone
`simulation-chain` binary of `chain/`, which builds where acpu does not).
It answers the launch question — does the network take off, does it hang,
is it secure — by running the **real soft3 kernel** in many nodes at once:

- **nodes**: N in-process nodes, each the kernel behind the wire's port
  (`soft3::chain::net::kernel::KernelChain`, its fjall store on disk), the
  real mempool (`net::mempool`), real chain-wire frames encoded and decoded
  by `net::frames`, and the protocol of `net/gossip.rs`, `net/sync.rs`,
  `net/serve.rs` re-expressed as event handlers: header-first gossip and
  relay, body fetch from the announcer, header sync by locator, ranged body
  download, the orphan and future-drift holds, peer scoring with decay and
  bans. Not real: QUIC streams (a request/response pair), the iroh-gossip
  mesh (broadcast to links), the wall-clock token buckets, discovery.
- **network**: per-link latency, jitter, loss and bandwidth (a
  serialization queue per direction), partitions and heals, links taken
  down (eclipse, crash), late joiners doing initial block download, nodes
  crashed and reopened from disk (the kernel replays), per-node clock skew.
- **clock**: a discrete-event loop on a virtual clock (ms); every tie
  broken by insertion order, every map ordered, the exponential sampler
  integer-only — a seed reproduces a run bit for bit (`determinism`).
- **mining oracle**: each miner finds its next block after an exponential
  time of mean `work(target) / hashrate`; the block is then *really*
  ground — a nonce whose hemera id meets the ASERT target — and goes
  through the same validation as every other block. Test networks use a
  2^248 genesis target (≈ 256 hashes a block) with the `sim` feature's
  relaxed `POW_LIMIT` (2^255); ASERT is scale-invariant, so its dynamics
  are the launch's. The `launch` suite uses the real genesis files at the
  real 2^232 target and grinds every block for real (Metal with
  `--features metal`, else every CPU core).
- **agents**: honest traffic, a flood at the fee floor, hostile peers
  (junk frames, oversize, bad signatures, replays, cross-chain replays,
  claim abuse, bodies on gossip, bad-work headers, wrong-chain status),
  a forger of valid-work blocks with broken bodies, selfish (SM1) and
  private-fork miners, an eclipser, and the economy's strategies
  (diverse stakers, concentration, recycler, hedger, Sybil splitter,
  self-linking ring, last-block copy, −1 griefer, payout-owner change).
- **invariants**, sampled continuously and at the end: liveness (block
  interval against 30 s, no stall), safety (honest nodes agree below depth
  120, convergence after quiesce, no reorg beyond 119), conservation at
  every block on every node, determinism (same seed, and every node's epoch
  records equal), resources (apply time, mempool upkeep, mempool capacity,
  RSS), the economy bounds of soft3 specs/chain-economy.md §5–§9
  recomputed from the chain (Δs per card, the 1 % cap, `P ≤ B = ⌊F/4⌋`).
  Known open findings run as **expected red**: a green there means the
  finding was fixed.

Suites: `gate` (≤ 10 min on an M4 Max; CI on Linux on every push, every
soft3 merge and every six hours; the soft3 release train's
`simulation-gate`), `nightly` (hours: larger N, longer horizons, the
stronger adversaries), `launch` (the genesis day of pussy and bostrom).
`simulation chain --list` prints them. Output: one JSONL file per scenario
and a markdown verdict table (scenario · invariant · verdict · evidence).

```bash
# a stand: soft3 + the sibling revisions it builds against (stand.toml)
python3 scripts/stand.py --into /tmp/stand && cd /tmp/stand/simulation
cargo build --profile sim -p simulation-chain
./target/sim/simulation-chain gate --out /tmp/sim-out --md /tmp/sim-out/verdict.md
cargo build --profile sim -p simulation-chain --features metal   # macOS, for `launch`
./target/sim/simulation-chain launch --out /tmp/sim-out
```

Receipts: `audit/network-2026-10.md`.

license: cyber license — don't trust. don't fear. don't beg.
