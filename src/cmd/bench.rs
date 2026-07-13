//! CLI benchmark — generate → pack (in-process) → measure.
//!
//! Invoked via `hexz bench`.

use crate::cmd::pack::{self, PackOptions};
use hexz_k::bench;
use std::time::Instant;

/// Run the full benchmark: generate data, pack, measure reads.
pub fn run() -> anyhow::Result<()> {
    println!("══════════════════════════════════════════════");
    println!("  hexz_k Benchmark");
    println!("══════════════════════════════════════════════\n");

    let work_dir = std::env::temp_dir().join("hexz_bench");
    if let Err(e) = std::fs::remove_dir_all(&work_dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("  warn: cleanup failed: {e}");
    }
    std::fs::create_dir_all(&work_dir)?;
    let input_dir = work_dir.join("input");
    let output_dir = work_dir.join("output");
    std::fs::create_dir_all(&input_dir)?;
    std::fs::create_dir_all(&output_dir)?;

    println!("Generating test data...");
    let fsize = bench::generate_game_test_files(&input_dir)?;
    println!(
        "  {:.1} MiB (scenario, background, voice, and UI assets)\n",
        fsize as f64 / 1_048_576.0
    );

    const ROUNDS: usize = 3;
    let mut best_read = 0f64;
    let mut best_iops = 0f64;

    for (comp, bs) in bench::BENCH_CONFIGS {
        let label = format!("{comp} {}KiB", bs / 1024);
        let archive = output_dir.join(format!("bench_{comp}_{bs}.hxz"));
        let archive_str = archive.to_string_lossy().to_string();
        let dir_str = input_dir.to_string_lossy().to_string();

        let mut sum_pack = 0u128;
        let mut sum_seq = 0f64;
        let mut sum_iops = 0f64;
        let mut archive_size = 0u64;
        let mut ratio = 0f64;

        for _ in 0..ROUNDS {
            let t0 = Instant::now();
            pack::pack_directory(&PackOptions {
                input: dir_str.clone(),
                output: archive_str.clone(),
                compression: comp.to_string(),
                encrypt: false,
                block_size: *bs,
                password: None,
            })?;
            sum_pack += t0.elapsed().as_millis();
            archive_size = std::fs::metadata(&archive)?.len();
            ratio = fsize as f64 / archive_size as f64;

            let (seq, iops) = bench::measure_reads(&archive)?;
            sum_seq += seq;
            sum_iops += iops;
        }

        let avg_pack = sum_pack / ROUNDS as u128;
        let avg_seq = sum_seq / ROUNDS as f64;
        let avg_iops = sum_iops / ROUNDS as f64;

        println!(
            "  {label}: pack {:>5}ms  {:>5.1} KiB  {:>4.1}x  |  seq {:>8.1} MB/s  |  IOPS {:>8.0}",
            avg_pack,
            archive_size as f64 / 1024.0,
            ratio,
            avg_seq,
            avg_iops,
        );
        best_read = best_read.max(avg_seq);
        best_iops = best_iops.max(avg_iops);
    }

    println!("\n──────────────────────────────────────────────");
    println!(
        "  Total test data: {:.1} MiB (mixed game assets)",
        fsize as f64 / 1_048_576.0
    );
    println!("  Best sequential: {:.1} MB/s", best_read);
    println!("  Best IOPS: {:.0}", best_iops);

    if let Err(e) = std::fs::remove_dir_all(&work_dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("  warn: cleanup failed: {e}");
    }
    println!("══════════════════════════════════════════════");
    Ok(())
}
