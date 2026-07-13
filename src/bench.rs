//! Shared benchmark utilities — used by both CLI and GUI bench.

use crate::ResourcePack;

fn fast_random_seeded(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
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

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E3779B97F4A7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D049BB133111EB);
    value ^ (value >> 31)
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

/// Legacy single-file benchmark specification.
pub const BENCH_SPECS: &[(&str, usize, &str)] = &[("data.bin", 33_554_432, "text")];

/// Visual-novel/game workload groups:
/// (directory, filename prefix, file count, bytes per file, content kind).
pub const GAME_BENCH_GROUPS: &[(&str, &str, usize, usize, &str)] = &[
    ("scenario", "scene", 256, 8_192, "text"),
    ("background", "bg", 48, 262_144, "random"),
    ("voice", "voice", 192, 65_536, "random"),
    ("ui", "sprite", 128, 16_384, "text"),
];

/// Generate all test data files into `dir`. Returns total raw bytes written.
pub fn generate_test_files(dir: &std::path::Path) -> anyhow::Result<u64> {
    let mut total = 0;
    for (name, size, kind) in BENCH_SPECS {
        let data = if *kind == "text" {
            lorem(*size)
        } else {
            fast_random_seeded(*size, 0xDEADBEEF_CAFEBABE)
        };
        std::fs::write(dir.join(name), data)?;
        total += *size as u64;
    }
    Ok(total)
}

/// Generate a mixed visual-novel/game workload into `dir`.
pub fn generate_game_test_files(dir: &std::path::Path) -> anyhow::Result<u64> {
    let mut total: u64 = 0;
    let mut seed = 0xDEADBEEF_CAFEBABEu64;
    for (group, prefix, count, size, kind) in GAME_BENCH_GROUPS {
        let group_dir = dir.join(group);
        std::fs::create_dir_all(&group_dir)?;
        for index in 0..*count {
            seed = splitmix64(seed);
            let data = if *kind == "text" {
                let mut data = lorem(*size);
                let marker = format!("{group}/{prefix}-{index:04}");
                let marker = marker.as_bytes();
                let marker_len = marker.len().min(data.len());
                data[..marker_len].copy_from_slice(&marker[..marker_len]);
                data
            } else {
                fast_random_seeded(*size, seed)
            };
            std::fs::write(group_dir.join(format!("{prefix}-{index:04}.bin")), data)?;
            total += *size as u64;
        }
    }
    Ok(total)
}

/// Measure sequential read throughput and random IOPS from an archive.
/// Returns (seq_mbps, iops).
pub fn measure_reads(archive_path: &std::path::Path) -> anyhow::Result<(f64, f64)> {
    use std::time::Instant;
    let pack = ResourcePack::open(archive_path, None)?;
    let files = pack.list_files();
    if files.is_empty() {
        anyhow::bail!("Cannot benchmark an archive with no files");
    }

    // Sequential
    let t = Instant::now();
    let mut read_bytes: u64 = 0;
    for f in &files {
        let data = pack.read_file(f)?;
        read_bytes += data.len() as u64;
    }
    let elapsed = t.elapsed().as_secs_f64().max(f64::EPSILON);
    let seq_mbps = read_bytes as f64 / elapsed / 1_048_576.0;

    // IOPS
    let n = files.len();
    let handles: Vec<_> = files
        .iter()
        .map(|path| pack.open_file(path))
        .collect::<anyhow::Result<_>>()?;
    const READ_SIZE: usize = 4096;
    let count = 2_000;
    let mut buffer = [0u8; READ_SIZE];
    let t = Instant::now();
    for i in 0..count {
        let file = &handles[i % n];
        // Knuth's multiplicative constant provides deterministic, well-spread
        // offsets while keeping the benchmark reproducible.
        let file_size = file.len();
        let offset = if file_size > READ_SIZE {
            i.wrapping_mul(2_654_435_761usize) % (file_size - READ_SIZE + 1)
        } else {
            0
        };
        let read = file.read_range_into(offset, &mut buffer)?;
        std::hint::black_box(&buffer[..read]);
    }
    let iops = count as f64 / t.elapsed().as_secs_f64().max(f64::EPSILON);

    Ok((seq_mbps, iops))
}
