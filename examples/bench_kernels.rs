//! kernel throughput on this machine — the baseline the honeycrisp
//! backend must beat, and its first NEON measurement.
//! run: cargo run --release --example bench_kernels

use std::time::Instant;

use simulation::kernels::{Kernels, Scalar};

fn bench<F: FnMut()>(name: &str, bytes: usize, mut f: F) {
    f(); // warm
    let t0 = Instant::now();
    let iters = 20;
    for _ in 0..iters {
        std::hint::black_box(&mut f);
        f();
    }
    let dt = t0.elapsed().as_secs_f64() / iters as f64;
    println!(
        "{:<28} {:>10.3} ms  {:>8.2} GB/s",
        name,
        dt * 1e3,
        bytes as f64 / dt / 1e9
    );
}

fn bench_scalar() {
    let kern = Scalar;
    let n = 1_000_000;
    let d = 64;

    let mut e = vec![0.1f32; n * d];
    let mut x = vec![0.2f32; d];
    let mut out = vec![0.0f32; n];
    for (i, v) in e.iter_mut().enumerate() {
        *v = ((i * 2654435761usize) % 1000) as f32 * 1e-6;
    }
    x[3] = 0.7;
    bench("gemv_rows n=1M d=64", n * d * 4, || {
        kern.gemv_rows(&e, n, d, &x, &mut out);
    });

    let a = vec![0.3f32; 4096];
    let mut y = vec![0.1f32; 4096];
    bench("dot+axpy 4K (ortho cell)", 4096 * 4 * 3, || {
        let d0 = kern.dot(&a, &y);
        kern.axpy(&mut y, -d0, &a);
    });

    let deg = 3usize;
    let row: Vec<u32> = (0..=n).map(|i| (i * deg) as u32).collect();
    let col: Vec<u32> = (0..n * deg).map(|i| (i % n) as u32).collect();
    let xv = vec![0.2f32; n];
    let mut so = vec![0.0f32; n];
    bench("spmv n=1M deg=3", n * deg * 4, || {
        for i in 0..n {
            let mut s = 0.0f32;
            for &j in &col[row[i] as usize..row[i + 1] as usize] {
                s += xv[j as usize];
            }
            so[i] = s;
        }
    });

    let a64: Vec<f32> = (0..65_536).map(|i| (i % 97) as f32 * 0.01).collect();
    let b64: Vec<f32> = (0..65_536).map(|i| (i % 89) as f32 * 0.01).collect();
    bench("dot 64K (scalar)", 65_536 * 4 * 2, || {
        std::hint::black_box(kern.dot(&a64, &b64));
    });
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn bench_neon() {
    use simulation::kernels::Backend;
    let n = 65_536;
    let a: Vec<f32> = (0..n).map(|i| (i % 97) as f32 * 0.01).collect();
    let b: Vec<f32> = (0..n).map(|i| (i % 89) as f32 * 0.01).collect();
    let scalar = Backend::Scalar.kernels();
    let neon = Backend::parse("neon").kernels();
    let t0 = Instant::now();
    let mut acc = 0.0f32;
    for _ in 0..2000 {
        acc += scalar.dot(&a, &b);
    }
    let ts = t0.elapsed().as_secs_f64() / 2000.0;
    let t0 = Instant::now();
    let mut acc2 = 0.0f32;
    for _ in 0..2000 {
        acc2 += neon.dot(&a, &b);
    }
    let tn = t0.elapsed().as_secs_f64() / 2000.0;
    std::hint::black_box(acc);
    std::hint::black_box(acc2);
    println!(
        "dot 64K  scalar {:>8.3} us | acpu NEON {:>8.3} us | speedup {:.2}x",
        ts * 1e6,
        tn * 1e6,
        ts / tn
    );
}

fn main() {
    bench_scalar();
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    bench_neon();
    println!("note: GB/s is bytes touched; spmv is latency-bound, expect low.");
}
