//! Internal benchmark for ResourcePack random read IOPS
//! Usage: cargo run --bin hexz_bench -- <path_to_hxz>

use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "../WebGAL_k/src-tauri/game.hxz".into());
    println!("Opening {} ...", path);
    let pack = hexz_k::ResourcePack::open(&path, None)?;

    let files: Vec<&str> = pack.list_files();
    println!("{} files indexed", files.len());

    // Pick 20 random files for testing
    let test_files: Vec<&&str> = files.iter().filter(|f| !f.is_empty()).collect();
    let n = test_files.len().min(20);

    // WARMUP
    for i in 0..n {
        let _ = pack.read_file(test_files[i]);
    }

    // COLD: drop filesystem cache (best effort)
    #[cfg(target_os = "linux")]
    { std::process::Command::new("sh").arg("-c").arg("echo 3 > /proc/sys/vm/drop_caches").output().ok(); }

    // LARGE FILE cold read
    let large = test_files.iter().find(|f| f.ends_with(".webm") || f.ends_with(".png")).unwrap_or(&&test_files[0]);
    let t0 = Instant::now();
    let data = pack.read_file(large)?;
    let cold_ms = t0.elapsed().as_millis();
    println!("Cold read {} ({:.2} MB): {} ms", large, data.len() as f64 / 1_048_576.0, cold_ms);

    // Warm random IOPS (1000 reads)
    let count = 1000;
    let t0 = Instant::now();
    for i in 0..count {
        let f = test_files[i % n];
        let _ = pack.read_file(f);
    }
    let elapsed = t0.elapsed();
    let iops = count as f64 / elapsed.as_secs_f64();
    println!("Warm random reads: {} in {:.2}s = {:.0} IOPS", count, elapsed.as_secs_f64(), iops);

    // Mixed random IOPS (different files each time)
    let t0 = Instant::now();
    for i in 0..count {
        let f = test_files[(i * 7 + 3) % n];
        let _ = pack.read_file(f);
    }
    let elapsed = t0.elapsed();
    let iops = count as f64 / elapsed.as_secs_f64();
    println!("Mixed random reads: {} in {:.2}s = {:.0} IOPS", count, elapsed.as_secs_f64(), iops);

    Ok(())
}
