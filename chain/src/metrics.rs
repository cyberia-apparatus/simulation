//! JSONL metrics of a run, and a digest of everything written (the
//! determinism check compares two runs' digests).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub struct Metrics {
    out: Option<BufWriter<File>>,
    hasher: Vec<u8>,
    pub lines: u64,
}

impl Metrics {
    pub fn to(path: Option<&Path>) -> Metrics {
        let out = path.and_then(|p| {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            File::create(p).ok().map(BufWriter::new)
        });
        Metrics { out, hasher: Vec::new(), lines: 0 }
    }

    pub fn emit(&mut self, v: serde_json::Value) {
        let line = v.to_string();
        // the digest skips wall-clock fields (rss, timings) so it is reproducible
        let mut stable = v.clone();
        if let Some(o) = stable.as_object_mut() {
            for k in ["rss_mb", "wall_ms", "apply_us", "wall_s"] {
                o.remove(k);
            }
        }
        let s = stable.to_string();
        let h = hemera::hash(&[self.hasher.as_slice(), s.as_bytes()].concat());
        self.hasher = h.as_bytes().to_vec();
        self.lines += 1;
        if let Some(o) = self.out.as_mut() {
            let _ = writeln!(o, "{line}");
        }
    }

    pub fn digest(&self) -> Vec<u8> {
        self.hasher.clone()
    }

    pub fn flush(&mut self) {
        if let Some(o) = self.out.as_mut() {
            let _ = o.flush();
        }
    }
}
