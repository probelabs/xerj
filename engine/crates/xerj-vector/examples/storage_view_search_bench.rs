//! Controlled release-mode A/B harness for the HNSW storage-read seam.
//!
//! Build one graph once, then point both the pre-view and candidate binaries
//! at those exact bytes:
//!
//! ```text
//! storage_view_search_bench build GRAPH NODES DIM
//! storage_view_search_bench search GRAPH QUERIES EF
//! ```

use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use xerj_vector::{DistanceMetric, HnswIndex, HnswParams};

struct Deterministic(u64);

impl Deterministic {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as f64 / 2_147_483_648.0) as f32 - 1.0
    }

    fn vector(&mut self, dimension: usize) -> Vec<f32> {
        (0..dimension).map(|_| self.next()).collect()
    }
}

fn parse_usize(value: Option<String>, name: &str) -> usize {
    value
        .unwrap_or_else(|| panic!("missing {name}"))
        .parse()
        .unwrap_or_else(|_| panic!("{name} must be an unsigned integer"))
}

fn build(path: &Path, nodes: usize, dimension: usize) {
    let index = HnswIndex::new(HnswParams::new(dimension, DistanceMetric::Cosine));
    let mut random = Deterministic(0x5eed_cafe_dead_beef);
    for node in 0..nodes {
        let external_id = (node as u64).wrapping_mul(17).wrapping_add(1);
        index
            .insert(external_id, random.vector(dimension))
            .expect("benchmark graph insert");
    }
    index.save_to(path).expect("benchmark graph save");
    println!("built_nodes={nodes} dimension={dimension}");
}

fn search(path: &Path, queries: usize, ef: usize) {
    let index = HnswIndex::load_from(path).expect("benchmark graph load");
    let dimension = index.params().dim;
    let mut random = Deterministic(0x1234_5678_9abc_def0);
    let query_vectors: Vec<Vec<f32>> = (0..queries.max(1_000))
        .map(|_| random.vector(dimension))
        .collect();

    for query in query_vectors.iter().take(1_000) {
        black_box(index.search(black_box(query), 10, ef).expect("warm search"));
    }

    let started = Instant::now();
    let mut checksum = 0u64;
    for query in query_vectors.iter().take(queries) {
        let hits = index
            .search(black_box(query), 10, ef)
            .expect("measured search");
        checksum = checksum.wrapping_add(hits.iter().fold(0u64, |sum, (id, distance)| {
            sum.wrapping_add(*id ^ u64::from(distance.to_bits()))
        }));
        black_box(&hits);
    }
    let elapsed = started.elapsed();
    println!(
        "queries={queries} ef={ef} elapsed_ns={} qps={:.3} checksum={checksum}",
        elapsed.as_nanos(),
        queries as f64 / elapsed.as_secs_f64()
    );
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let operation = arguments
        .next()
        .expect("expected build or search operation");
    let path = arguments.next().expect("missing graph path");
    match operation.as_str() {
        "build" => build(
            Path::new(&path),
            parse_usize(arguments.next(), "nodes"),
            parse_usize(arguments.next(), "dimension"),
        ),
        "search" => search(
            Path::new(&path),
            parse_usize(arguments.next(), "queries"),
            parse_usize(arguments.next(), "ef"),
        ),
        _ => panic!("operation must be build or search"),
    }
}
