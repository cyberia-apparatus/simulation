//! hot kernels behind one trait — scalar f32 now, honeycrisp (NEON/AMX)
//! behind the same seams later. buffers are plain row-major f32 vecs;
//! NEON unaligned loads on aarch64 are cheap, so no alignment ceremony.

/// vector ops the compile and the scorer are built from.
pub trait Kernels {
    /// dot(a, b)
    fn dot(&self, a: &[f32], b: &[f32]) -> f32;
    /// y += a * x
    fn axpy(&self, y: &mut [f32], a: f32, x: &[f32]);
    /// scores = E @ x over n rows of width d (row-major E, n*d)
    fn gemv_rows(&self, e: &[f32], n: usize, d: usize, x: &[f32], out: &mut [f32]);
    /// rms-norm in place: x / sqrt(mean(x^2) + eps)
    fn rmsnorm(&self, x: &mut [f32], eps: f32);
}

#[derive(Default, Clone, Copy)]
pub struct Scalar;

impl Kernels for Scalar {
    fn dot(&self, a: &[f32], b: &[f32]) -> f32 {
        debug_assert_eq!(a.len(), b.len());
        let mut s = 0.0f32;
        for i in 0..a.len() {
            s += a[i] * b[i];
        }
        s
    }

    fn axpy(&self, y: &mut [f32], a: f32, x: &[f32]) {
        debug_assert_eq!(y.len(), x.len());
        for i in 0..y.len() {
            y[i] += a * x[i];
        }
    }

    fn gemv_rows(&self, e: &[f32], n: usize, d: usize, x: &[f32], out: &mut [f32]) {
        debug_assert_eq!(e.len(), n * d);
        debug_assert_eq!(x.len(), d);
        debug_assert_eq!(out.len(), n);
        for i in 0..n {
            out[i] = self.dot(&e[i * d..(i + 1) * d], x);
        }
    }

    fn rmsnorm(&self, x: &mut [f32], eps: f32) {
        let ms = self.dot(x, x) / x.len() as f32;
        let inv = 1.0 / (ms + eps).sqrt();
        for v in x.iter_mut() {
            *v *= inv;
        }
    }
}

/// Apple Silicon NEON backend: dot through honeycrisp's acpu; the
/// remaining ops delegate to scalar until their NEON ops land
/// (axpy is the documented next one — see FINDINGS.md hotspot map).
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[derive(Default, Clone, Copy)]
pub struct Neon;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl Kernels for Neon {
    fn dot(&self, a: &[f32], b: &[f32]) -> f32 {
        acpu::vector::dot(a, b)
    }

    fn axpy(&self, y: &mut [f32], a: f32, x: &[f32]) {
        Scalar.axpy(y, a, x)
    }

    fn gemv_rows(&self, e: &[f32], n: usize, d: usize, x: &[f32], out: &mut [f32]) {
        Scalar.gemv_rows(e, n, d, x, out)
    }

    fn rmsnorm(&self, x: &mut [f32], eps: f32) {
        Scalar.rmsnorm(x, eps)
    }
}

/// backend selector for the CLI.
pub enum Backend {
    Scalar,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    Neon,
}

impl Backend {
    pub fn parse(s: &str) -> Backend {
        match s {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            "neon" => Backend::Neon,
            _ => Backend::Scalar,
        }
    }

    pub fn kernels(&self) -> Box<dyn Kernels> {
        match self {
            Backend::Scalar => Box::new(Scalar),
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Backend::Neon => Box::new(Neon),
        }
    }
}
