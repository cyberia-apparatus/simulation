//! Deterministic randomness: xoshiro256** seeded by splitmix64, and an
//! integer-only exponential sampler (no `f64::ln`, whose last bit may differ
//! between libms), so a seed reproduces a run bit for bit on any machine.

#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
}

fn splitmix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut x = seed;
        Rng { s: [splitmix(&mut x), splitmix(&mut x), splitmix(&mut x), splitmix(&mut x)] }
    }

    /// An independent stream for a named consumer (a node, a link, an agent).
    pub fn fork(&mut self, tag: u64) -> Rng {
        let mut x = self.next() ^ tag.wrapping_mul(0xD6E8_FEB8_6659_FD93);
        Rng { s: [splitmix(&mut x), splitmix(&mut x), splitmix(&mut x), splitmix(&mut x)] }
    }

    pub fn next(&mut self) -> u64 {
        let r = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        r
    }

    /// Uniform in `[0, n)`; `n = 0` gives 0.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        ((self.next() as u128 * n as u128) >> 64) as u64
    }

    /// Uniform in `[lo, hi]`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    /// True with probability `ppm / 10^6`.
    pub fn chance_ppm(&mut self, ppm: u64) -> bool {
        self.below(1_000_000) < ppm
    }

    pub fn bytes32(&mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for c in out.chunks_exact_mut(8) {
            c.copy_from_slice(&self.next().to_le_bytes());
        }
        out
    }

    /// An exponential variate with the given mean (same unit), integers only.
    pub fn exp(&mut self, mean: u64) -> u64 {
        let u = self.next() | 1; // (0, 2^64)
        let x = neg_ln_q32(u); // −ln(u / 2^64) at 2^32
        ((mean as u128 * x as u128) >> 32).min(u64::MAX as u128) as u64
    }

    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            v.swap(i, j);
        }
    }
}

/// `log2(u)` at 2^32 for `u ≥ 1`, by repeated squaring of the mantissa
/// (exact to the last fractional bit kept).
pub fn log2_q32(u: u64) -> u64 {
    debug_assert!(u > 0);
    let ip = 63 - u.leading_zeros() as u64;
    // mantissa in [1, 2) at 2^63
    let mut m: u128 = (u as u128) << (63 - ip);
    let mut frac = 0u64;
    for i in 1..=32 {
        m = (m * m) >> 63;
        if m >= 1u128 << 64 {
            m >>= 1;
            frac |= 1u64 << (32 - i);
        }
    }
    (ip << 32) | frac
}

/// `−ln(u / 2^64)` at 2^32.
pub fn neg_ln_q32(u: u64) -> u64 {
    const LN2_Q32: u128 = 2_977_044_472; // ln 2 · 2^32
    let x = (64u128 << 32) - log2_q32(u) as u128;
    ((x * LN2_Q32) >> 32) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log2_exact_on_powers_and_close_elsewhere() {
        assert_eq!(log2_q32(1), 0);
        assert_eq!(log2_q32(1 << 40), 40 << 32);
        // log2(3) = 1.584962500721156
        let l = log2_q32(3) as f64 / (1u64 << 32) as f64;
        assert!((l - 1.584962500721156).abs() < 1e-9, "{l}");
    }

    #[test]
    fn exponential_mean_is_right() {
        let mut r = Rng::new(1);
        let n = 200_000u64;
        let sum: u64 = (0..n).map(|_| r.exp(30_000)).sum();
        let mean = sum / n;
        assert!((29_700..=30_300).contains(&mean), "{mean}");
    }

    #[test]
    fn seeds_reproduce() {
        let a: Vec<u64> = { let mut r = Rng::new(9); (0..8).map(|_| r.next()).collect() };
        let b: Vec<u64> = { let mut r = Rng::new(9); (0..8).map(|_| r.next()).collect() };
        assert_eq!(a, b);
    }
}
