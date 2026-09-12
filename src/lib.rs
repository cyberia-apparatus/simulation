pub mod graph;
pub mod kernels;
pub mod metrics;
pub mod model;
pub mod rng;
pub mod spectrum;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernels::Kernels;

    /// tiny graph -> compile -> E is finite, sigma positive. guards the
    /// full pipeline against regressions in growth, pagerank, and the
    /// subspace iteration.
    #[test]
    fn compiles_a_tiny_graph() {
        let mut g = graph::Graph::new(64);
        for _ in 0..16 {
            g.add_node();
        }
        // ring + chords: connected, non-degenerate
        for i in 0..16u32 {
            g.link(i, (i + 1) % 16);
            g.link(i, (i + 5) % 16);
        }
        let mut rng = rng::Rng::new(7);
        let spec = spectrum::Spectrum::new(12);
        let emb = spec.compile(&g, 16, &mut rng, &kernels::Scalar);
        assert_eq!(emb.n, 16);
        assert!(emb.e.iter().all(|x| x.is_finite()));
        assert!(emb.sigma[0] > 0.0);
        assert!(emb.sigma[0] >= emb.sigma[emb.sigma.len() - 1]);
    }

    #[test]
    fn rmsnorm_unit_rms() {
        let k = kernels::Scalar;
        let mut x = vec![3.0f32, 4.0];
        k.rmsnorm(&mut x, 1e-5);
        let ms = k.dot(&x, &x) / x.len() as f32;
        assert!((ms - 1.0).abs() < 1e-3);
    }

    #[test]
    fn gemv_rows_matches_dots() {
        let k = kernels::Scalar;
        let d = 8;
        let n = 4;
        let e: Vec<f32> = (0..n * d).map(|i| i as f32 * 0.1).collect();
        let x: Vec<f32> = (0..d).map(|i| i as f32 * 0.01).collect();
        let mut out = vec![0.0f32; n];
        k.gemv_rows(&e, n, d, &x, &mut out);
        for i in 0..n {
            let expect = k.dot(&e[i * d..(i + 1) * d], &x);
            assert!((out[i] - expect).abs() < 1e-5);
        }
    }
}
