//! the compile: E = U sqrt(Sigma) of the shipped mixed operator
//! M = D(A + 0.5 A^2) D — same math as tru's pass 3/4, f32, and the
//! kernels trait instead of scalar loops. block is column-major (k
//! contiguous columns of length n) so dot/axpy stream.

use crate::graph::Graph;
use crate::kernels::Kernels;
use crate::rng::Rng;

const HOP2: f32 = 0.5;
const ALPHA: f32 = 0.85;

pub struct Embedding {
    /// row-major n×d
    pub e: Vec<f32>,
    pub n: usize,
    pub d: usize,
    pub sigma: Vec<f32>,
}

pub struct Spectrum {
    pub iters: usize,
}

/// preferential pagerank: dangling mass returns to the prior (tru#1).
fn pagerank(g: &Graph, csr: &(Vec<u32>, Vec<u32>)) -> Vec<f32> {
    let n = g.n;
    let (row, col) = csr;
    let mut phi = vec![1.0f32 / n as f32; n];
    let mut next = vec![0.0f32; n];
    let out_strength: Vec<f32> = (0..n).map(|i| (row[i + 1] - row[i]) as f32).collect();
    for _ in 0..200 {
        let mut dmass = 0.0f32;
        for i in 0..n {
            if out_strength[i] == 0.0 {
                dmass += phi[i];
            }
        }
        for slot in next.iter_mut() {
            // P @ phi: column-stochastic accumulate
            *slot = 0.0;
        }
        for i in 0..n {
            if out_strength[i] > 0.0 {
                let w = ALPHA * phi[i] / out_strength[i];
                for &j in &col[row[i] as usize..row[i + 1] as usize] {
                    next[j as usize] += w;
                }
            }
        }
        let tele = (1.0 - ALPHA) / n as f32;
        let dm = ALPHA * dmass;
        let mut sum = 0.0f32;
        for j in 0..n {
            next[j] += dm * phi[j] + tele;
            sum += next[j];
        }
        let mut diff = 0.0f32;
        for j in 0..n {
            next[j] /= sum;
            diff += (next[j] - phi[j]).abs();
        }
        std::mem::swap(&mut phi, &mut next);
        if diff < 1e-7 {
            break;
        }
    }
    phi
}

fn spmv(row: &[u32], col: &[u32], x: &[f32], out: &mut [f32]) {
    for i in 0..out.len() {
        let mut s = 0.0f32;
        for &j in &col[row[i] as usize..row[i + 1] as usize] {
            s += x[j as usize];
        }
        out[i] = s;
    }
}

impl Spectrum {
    pub fn new(iters: usize) -> Self {
        Spectrum { iters }
    }

    /// compile the graph into E. memory: 2 column-major blocks
    /// (2 * k * n * 4 bytes) + E (n * d * 4). at n=10M, k=d=64 that is
    /// ~7.7 GB — rungs above ~4M nodes want the chunked variant (later).
    pub fn compile(&self, g: &Graph, d: usize, rng: &mut Rng, kern: &dyn Kernels) -> Embedding {
        let k = d;
        let n = g.n;
        let (row, col) = g.csr();
        let phi = pagerank(g, &(row.clone(), col.clone()));
        let ds: Vec<f32> = phi.iter().map(|p| p.sqrt()).collect();

        // transposed CSR for M^T actions
        let mut deg = vec![0usize; n];
        for &j in &col {
            deg[j as usize] += 1;
        }
        let mut trow = vec![0u32; n + 1];
        for i in 0..n {
            trow[i + 1] = trow[i] + deg[i] as u32;
        }
        let mut tcol = vec![0u32; col.len()];
        let mut fill = vec![0u32; n];
        for i in 0..n {
            for &j in &col[row[i] as usize..row[i + 1] as usize] {
                let jj = j as usize;
                tcol[(trow[jj] + fill[jj]) as usize] = i as u32;
                fill[jj] += 1;
            }
        }

        let apply_m = |x: &[f32], out: &mut [f32]| {
            let mut t = vec![0.0f32; n];
            let mut ax = vec![0.0f32; n];
            let mut a2x = vec![0.0f32; n];
            for i in 0..n {
                t[i] = ds[i] * x[i];
            }
            spmv(&row, &col, &t, &mut ax);
            spmv(&row, &col, &ax, &mut a2x);
            for i in 0..n {
                out[i] = ds[i] * (ax[i] + HOP2 * a2x[i]);
            }
        };
        let apply_mt = |x: &[f32], out: &mut [f32]| {
            let mut t = vec![0.0f32; n];
            let mut ax = vec![0.0f32; n];
            let mut a2x = vec![0.0f32; n];
            for i in 0..n {
                t[i] = ds[i] * x[i];
            }
            spmv(&trow, &tcol, &t, &mut ax);
            spmv(&trow, &tcol, &ax, &mut a2x);
            for i in 0..n {
                out[i] = ds[i] * (ax[i] + HOP2 * a2x[i]);
            }
        };

        let mut block = vec![0.0f32; k * n]; // column-major: col c at [c*n..(c+1)*n]
        let mut seed_col = |c: usize, v: &mut [f32]| {
            let mut r = Rng::new((c as u64 + 1).wrapping_mul(0x9E3779B97F4A7C15) ^ rng.next());
            for x in v.iter_mut() {
                *x = ((r.next() >> 40) as f32 / (1u32 << 24) as f32) - 0.5;
            }
        };
        for c in 0..k {
            seed_col(c, &mut block[c * n..(c + 1) * n]);
        }

        let ortho = |block: &mut [f32]| {
            // modified Gram-Schmidt, denominator cached (tru lesson);
            // abs-max normalize per column like the fixed-point pipeline.
            let mut denoms = vec![0.0f32; k];
            for j in 0..k {
                let (lo, hi) = (j * n, (j + 1) * n);
                for i in 0..j {
                    if denoms[i] == 0.0 {
                        continue;
                    }
                    let d = kern.dot(&block[i * n..(i + 1) * n], &block[lo..hi]) / denoms[i];
                    let (a, b) = block.split_at_mut(lo);
                    kern.axpy(&mut b[..n], -d, &a[i * n..(i + 1) * n]);
                }
                let mut m = 0.0f32;
                for &x in &block[lo..hi] {
                    m = m.max(x.abs());
                }
                if m > 0.0 {
                    for x in &mut block[lo..hi] {
                        *x /= m;
                    }
                }
                denoms[j] = kern.dot(&block[lo..hi], &block[lo..hi]);
            }
        };

        ortho(&mut block);
        let mut tmp = vec![0.0f32; n];
        let mut tmp2 = vec![0.0f32; n];
        for _ in 0..self.iters {
            for c in 0..k {
                apply_m(&block[c * n..(c + 1) * n], &mut tmp);
                apply_mt(&tmp, &mut tmp2);
                block[c * n..(c + 1) * n].copy_from_slice(&tmp2);
            }
            ortho(&mut block);
            ortho(&mut block);
        }

        // Rayleigh: sigma = ||M v||, u = M v / sigma; E rows = u sqrt(sigma)
        let mut e = vec![0.0f32; n * d];
        let mut sigma = vec![0.0f32; k];
        let mut u = vec![0.0f32; n];
        for c in 0..k {
            apply_m(&block[c * n..(c + 1) * n], &mut u);
            let s = kern.dot(&u, &u).sqrt();
            sigma[c] = s;
            let scale = s.sqrt();
            if s > 1e-12 {
                for x in u.iter_mut() {
                    *x /= s;
                }
            }
            for i in 0..n {
                e[i * d + c] = u[i] * scale;
            }
        }
        Embedding { e, n, d, sigma }
    }
}
