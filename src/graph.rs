//! growing sparse graph: append-only adjacency with degree-indexed
//! sampling for preferential attachment. CSR rebuilt per compile from
//! the edge log (edges are append-only, so the rebuild is O(nnz)).

pub struct Graph {
    pub n: usize,
    /// edge log: (src, tgt) in arrival order
    pub edges: Vec<(u32, u32)>,
    /// out-adjacency as appended (Vec per node via index)
    out: Vec<Vec<u32>>,
    /// total degree (out + in) per node — PA weight
    pub degree: Vec<u32>,
    pub max_degree: u32,
}

impl Graph {
    pub fn new(cap_nodes: usize) -> Self {
        Graph {
            n: 0,
            edges: Vec::with_capacity(cap_nodes * 2),
            out: Vec::with_capacity(cap_nodes),
            degree: Vec::with_capacity(cap_nodes),
            max_degree: 0,
        }
    }

    pub fn add_node(&mut self) -> u32 {
        let id = self.n as u32;
        self.n += 1;
        self.out.push(Vec::new());
        self.degree.push(0);
        id
    }

    pub fn link(&mut self, s: u32, t: u32) {
        self.edges.push((s, t));
        self.out[s as usize].push(t);
        self.degree[s as usize] += 1;
        self.degree[t as usize] += 1;
        if self.degree[s as usize] > self.max_degree {
            self.max_degree = self.degree[s as usize];
        }
        if self.degree[t as usize] > self.max_degree {
            self.max_degree = self.degree[t as usize];
        }
    }

    pub fn out_neighbors(&self, s: u32) -> &[u32] {
        &self.out[s as usize]
    }

    pub fn is_linked(&self, s: u32, t: u32) -> bool {
        self.out[s as usize].contains(&t)
    }

    /// CSR snapshot for the spectrum pass: (row_ptr, col_idx, values=all 1).
    /// maxw normalization is a no-op on binary graphs.
    pub fn csr(&self) -> (Vec<u32>, Vec<u32>) {
        let mut row = Vec::with_capacity(self.n + 1);
        let mut col = Vec::with_capacity(self.edges.len());
        row.push(0u32);
        for node in &self.out {
            col.extend_from_slice(node);
            row.push(col.len() as u32);
        }
        (row, col)
    }
}
