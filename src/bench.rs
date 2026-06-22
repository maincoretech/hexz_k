//! Shared benchmark utilities — used by both CLI and GUI bench.

use crate::ResourcePack;

/// Generate `len` bytes of deterministic pseudo-random data.
pub(crate) fn fast_random(len: usize) -> Vec<u8> {
    let mut state: u64 = 0xDEADBEEF_CAFEBABE;
    let mut out = vec![0u8; len];
    let chunks = len / 8;
    let remainder = len % 8;
    for i in 0..chunks {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let bytes = state.to_le_bytes();
        let base = i * 8;
        out[base..base + 8].copy_from_slice(&bytes);
    }
    if remainder > 0 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let bytes = state.to_le_bytes();
        let base = chunks * 8;
        out[base..].copy_from_slice(&bytes[..remainder]);
    }
    out
}

/// Generate `len` bytes of highly-compressible lorem-ipsum text.
pub(crate) fn lorem(len: usize) -> Vec<u8> {
    let words = b"Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. ";
    let wlen = words.len();
    let mut out = Vec::with_capacity(len);
    let full_copies = len / wlen;
    for _ in 0..full_copies {
        out.extend_from_slice(words);
    }
    let rem = len - out.len();
    if rem > 0 {
        out.extend_from_slice(&words[..rem]);
    }
    out
}

/// Standard benchmark configuration: (algorithm, block_size).
pub const BENCH_CONFIGS: &[(&str, u32)] = &[("lz4", 65536), ("zstd", 65536)];

/// Standard test data spec: (filename, size_bytes, kind).
pub const BENCH_SPECS: &[(&str, usize, &str)] = &[("data.bin", 33_554_432, "text")];

/// Generate all test data files into `dir`. Returns total raw bytes written.
pub fn generate_test_files(dir: &std::path::Path) -> anyhow::Result<u64> {
    let mut total: u64 = 0;
    for (name, size, kind) in BENCH_SPECS {
        let data: Vec<u8> = if *kind == "text" {
            lorem(*size)
        } else {
            fast_random(*size)
        };
        std::fs::write(dir.join(name), &data)?;
        total += *size as u64;
    }
    Ok(total)
}

/// Measure sequential read throughput and random IOPS from an archive.
/// Returns (seq_mbps, iops).
pub fn measure_reads(archive_path: &std::path::Path) -> anyhow::Result<(f64, f64)> {
    use std::time::Instant;
    let pack = ResourcePack::open(archive_path, None)?;
    let files: Vec<String> = pack.list_files().iter().map(|s| s.to_string()).collect();

    // Sequential
    let t = Instant::now();
    let mut read_bytes: u64 = 0;
    for f in &files {
        if let Ok(d) = pack.read_file(f) {
            read_bytes += d.len() as u64;
        }
    }
    let seq_mbps = read_bytes as f64 / t.elapsed().as_millis().max(1) as f64 * 1000.0 / 1_048_576.0;

    // IOPS
    let n = files.len().max(1);
    let count = 500;
    let t = Instant::now();
    for i in 0..count {
        let _ = pack.read_file(&files[i % n]);
    }
    let iops = count as f64 / t.elapsed().as_millis().max(1) as f64 * 1000.0;

    Ok((seq_mbps, iops))
}
