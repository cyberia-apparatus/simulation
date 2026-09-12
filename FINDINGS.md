# findings — the closed-loop emergence simulator

measured results live here; the simulator is this repo; it uses honeycrisp
(acpu NEON) for the hot kernels via `--kernels neon`.

## sweep 1 — loop coupling g at 60k nodes (2 seeds), forecast unseen-MRR

config: `--nodes 60000 --q 0.5 --d 64 --compile-every 8000 --probe 16000
--seen-probe 200`. the unseen metric is a forecast: 16k future links
reserved by the hidden rule, ranked by the freshly compiled model
before they enter the graph. raw runs in `runs/g*_s*.jsonl`.

| g | unseen MRR (s7 / s13) | seen MRR | sigma1/sigma_k |
|---|---|---|---|
| 0.0 (open loop) | 0.155 / 0.161 | 0.054 / 0.078 | 21 / 26 |
| 0.25 | 0.184 / 0.184 | 0.082 / 0.085 | ~4.4k |
| 0.5 | 0.167 / 0.171 | 0.100 / 0.110 | ~9k / ~21k |
| 0.75 | 0.152 / 0.143 | 0.126 / 0.135 | ~26k / ~37k |
| 1.0 | 0.122 | 0.162 | ~50k |

## the resonance insight

**emergence is not the maximum of the loop — it is the loop's
resonance.** closing the feedback (agents partly following the compiled
model's suggestions) measurably improves the predictability of FUTURE
links, but only at moderate coupling: g=0.25 is the optimum (+16%
unseen MRR over open loop), and full coupling g=1.0 LOSES the
advantage entirely.

mechanism, observed through the spectral stats: suggested links
sharpen the very geometry the model reads (sigma1/sigma_k climbs from
~20 to ~50k as g -> 1). a little sharpening makes the graph more
legible; too much collapses it toward a low-dimensional echo of the
model itself, diluting the hidden rule — the independent physics the
model cannot see. past the resonance point the loop optimizes the
model's reflection, not the world's structure.

second measured regularity: the seen/unseen crossover. memorization
MRR rises monotonically with g and overtakes generalization at
g >= 0.75 — the same disease as tru's ft_ablation (gradient training
memorizes, structure-only signal does not generalize), here reproduced
CONTROLLABLY in synthetic growth, and bounded by the same resonance.

## status and caveats

- one size (60k nodes), one hidden rule (PA + triadic closure, q=0.5),
  two seeds, forecast read at a single tick. the g* position is
  indicative, not fitted.
- the open question the sweeps must answer: how does the optimal
  coupling g*(n) move with graph size? if g*(n) falls with n, there is
  a scale where even a weak product loop (g ~ 0.01, the real fraction
  of suggestion-following links in cyb) crosses into runaway — that is
  the emergence scale of crystal/emergence.md.
- compute budget per run is compile-dominated (~6 s per tick scalar
  f32); scaling to millions of nodes wants the ortho on NEON/AMX
  (acpu), which is exactly why this crate lives in the honeycrisp
  workspace.
