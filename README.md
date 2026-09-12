# simulation

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
- ASCII-only, no external crates.

license: cyber license — don't trust. don't fear. don't beg.
