#![allow(dead_code)]
// Provided by the copy step in README.md -- goldenhnsw is not published.
mod goldenhnsw;

use goldenhnsw::{HnswIndex, HnswParams};
use hnsw_rs::prelude::*;
use std::time::Instant;

const N: usize = 20_000;
const DIM: usize = 128;
const QUERIES: usize = 200;
const K: usize = 10;

/// SplitMix64 -> deterministic data without a rand dependency.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Standard normal via Box-Muller, so vectors are isotropic rather than
    /// cube-shaped (uniform cubes make ANN look artificially easy).
    fn normal(&mut self) -> f32 {
        let u1 = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        let u2 = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        let u1 = u1.max(1e-12);
        ((-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()) as f32
    }
    fn unit_vec(&mut self, d: usize) -> Vec<f32> {
        let mut v: Vec<f32> = (0..d).map(|_| self.normal()).collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        for x in &mut v {
            *x /= norm;
        }
        v
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Exact top-K by inner product. The oracle.
fn brute_force(data: &[Vec<f32>], q: &[f32], k: usize) -> Vec<usize> {
    let mut scored: Vec<(usize, f32)> = data.iter().enumerate().map(|(i, v)| (i, dot(q, v))).collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    scored.into_iter().take(k).map(|(i, _)| i).collect()
}

fn recall(got: &[usize], truth: &[usize]) -> f64 {
    let hits = got.iter().filter(|g| truth.contains(g)).count();
    hits as f64 / truth.len() as f64
}

/// Vectors drawn near a set of centroids, then renormalised.
///
/// Real sentence embeddings are strongly clustered by topic; uniform vectors on
/// the sphere are not, and in 128 dimensions their pairwise distances
/// concentrate so hard that a navigable-graph index has almost nothing to
/// navigate. Benchmarking only on uniform data measures the pathological case
/// and understates every ANN index.
fn clustered(rng: &mut Rng, n: usize, dim: usize, centroids: usize, spread: f32) -> Vec<Vec<f32>> {
    let centers: Vec<Vec<f32>> = (0..centroids).map(|_| rng.unit_vec(dim)).collect();
    (0..n)
        .map(|i| {
            let c = &centers[i % centroids];
            let mut v: Vec<f32> = c
                .iter()
                .map(|x| x + spread * rng.normal() / (dim as f32).sqrt())
                .collect();
            let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
            for x in &mut v {
                *x /= norm;
            }
            v
        })
        .collect()
}

fn main() {
    for (label, spread) in [("uniform on the sphere", None), ("clustered (200 topics)", Some(0.35f32))] {
        run(label, spread);
        println!();
    }
}

fn run(label: &str, spread: Option<f32>) {
    let mut rng = Rng(42);
    let (data, queries): (Vec<Vec<f32>>, Vec<Vec<f32>>) = match spread {
        None => (
            (0..N).map(|_| rng.unit_vec(DIM)).collect(),
            (0..QUERIES).map(|_| rng.unit_vec(DIM)).collect(),
        ),
        Some(sp) => {
            let d = clustered(&mut rng, N, DIM, 200, sp);
            // Queries drawn from the same distribution: a real query looks like
            // the corpus, not like noise.
            let q = clustered(&mut rng, QUERIES, DIM, 200, sp);
            (d, q)
        }
    };
    println!("=== {label} ===");

    let t = Instant::now();
    let truth: Vec<Vec<usize>> = queries.iter().map(|q| brute_force(&data, q, K)).collect();
    let bf_total = t.elapsed();
    println!("N={N} dim={DIM} queries={QUERIES} k={K}");
    println!("{:<28} {:>10} {:>12} {:>10}", "backend", "build", "query(avg)", "recall@10");
    println!("{}", "-".repeat(64));
    println!(
        "{:<28} {:>10} {:>12} {:>10.3}",
        "brute force (exact)",
        "-",
        format!("{:.1?}", bf_total / QUERIES as u32),
        1.0
    );

    for ef in [50usize, 200] {
        // ---- goldenhnsw ----
        let t = Instant::now();
        let mut g = HnswIndex::new(
            DIM,
            HnswParams {
                m: 16,
                ef_construction: 200,
                ef_search: ef,
                seed: 42,
            },
        );
        for v in &data {
            g.add(v);
        }
        let g_build = t.elapsed();

        let t = Instant::now();
        let mut g_recall = 0.0;
        for (qi, q) in queries.iter().enumerate() {
            let got: Vec<usize> = g.search(q, K).into_iter().map(|(id, _)| id as usize).collect();
            g_recall += recall(&got, &truth[qi]);
        }
        let g_query = t.elapsed();
        g_recall /= QUERIES as f64;

        println!(
            "{:<28} {:>10} {:>12} {:>10.3}",
            format!("goldenhnsw (ef={ef})"),
            format!("{:.2?}", g_build),
            format!("{:.1?}", g_query / QUERIES as u32),
            g_recall
        );

        // ---- hnsw_rs ----
        let t = Instant::now();
        let h: Hnsw<f32, DistDot> = Hnsw::new(16, N, 16, 200, DistDot {});
        for (i, v) in data.iter().enumerate() {
            h.insert((v.as_slice(), i));
        }
        let h_build = t.elapsed();

        let t = Instant::now();
        let mut h_recall = 0.0;
        for (qi, q) in queries.iter().enumerate() {
            let got: Vec<usize> = h.search(q, K, ef).into_iter().map(|n| n.d_id).collect();
            h_recall += recall(&got, &truth[qi]);
        }
        let h_query = t.elapsed();
        h_recall /= QUERIES as f64;

        println!(
            "{:<28} {:>10} {:>12} {:>10.3}",
            format!("hnsw_rs (ef={ef})"),
            format!("{:.2?}", h_build),
            format!("{:.1?}", h_query / QUERIES as u32),
            h_recall
        );
    }
}
