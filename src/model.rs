//! the compiled model: score(v | s) = <rms(E_s), E_v> — the zero-layer
//! scorer that tru's evals measured as the strongest init signal. the
//! suggestion loop and the metrics both go through here, so this is the
//! hottest path in the closed loop.

use crate::kernels::{Kernels, Scalar};
use crate::spectrum::Embedding;

pub struct Model<'a> {
    pub emb: &'a Embedding,
    kern: Scalar,
    scores: Vec<f32>,
    q: Vec<f32>,
}

impl<'a> Model<'a> {
    pub fn new(emb: &'a Embedding) -> Self {
        Model {
            emb,
            kern: Scalar,
            scores: vec![0.0f32; emb.n],
            q: vec![0.0f32; emb.d],
        }
    }

    /// full scoring vector for source s: scores[v] = <rms(E_s), E_v>.
    /// s beyond the compiled n (graph grew since the compile) yields a
    /// zero vector — callers treat it as "no signal".
    pub fn score_all(&mut self, s: usize) -> &[f32] {
        if s >= self.emb.n {
            for v in self.scores.iter_mut() {
                *v = 0.0;
            }
            return &self.scores;
        }
        self.q
            .copy_from_slice(&self.emb.e[s * self.emb.d..(s + 1) * self.emb.d]);
        self.kern.rmsnorm(&mut self.q, 1e-5);
        self.kern.gemv_rows(
            &self.emb.e,
            self.emb.n,
            self.emb.d,
            &self.q,
            &mut self.scores,
        );
        &self.scores
    }

    /// best suggestion for s among candidates not already linked.
    pub fn suggest(&mut self, s: usize, linked: &[u32]) -> usize {
        let scores = self.score_all(s);
        let mut best = usize::MAX;
        let mut bv = f32::NEG_INFINITY;
        for (v, &sc) in scores.iter().enumerate() {
            if v == s || linked.contains(&(v as u32)) {
                continue;
            }
            if sc > bv {
                bv = sc;
                best = v;
            }
        }
        best
    }

    /// rank of target t in the scoring for s, excluding s's out-links
    /// and t itself (MRR-style). one pass over scores.
    pub fn rank_of(&mut self, s: usize, t: usize, linked: &[u32]) -> usize {
        if s >= self.emb.n || t >= self.emb.n {
            return self.emb.n + 1; // unrankable at this compile
        }
        let scores = self.score_all(s).to_vec();
        let st = scores[t];
        let mut above = 0usize;
        for (v, &sc) in scores.iter().enumerate() {
            if v == t {
                continue;
            }
            if sc > st && !linked.contains(&(v as u32)) {
                above += 1;
            }
        }
        1 + above
    }
}
