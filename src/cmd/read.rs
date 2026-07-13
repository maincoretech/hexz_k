//! Read/list/extract/show/preview — thin wrappers over hexz_k public API.

use anyhow::Context;
use hexz_k::ResourcePack;
use std::io::Write;

/// List all files in an archive.
pub fn list_files(archive_path: &str, password: Option<&str>) -> anyhow::Result<()> {
    let pack = ResourcePack::open(archive_path, password)?;
    let files = pack.list_files();
    println!("Archive: {}", archive_path);
    println!("  Size: {:.2} MB", pack.main_size() as f64 / 1_048_576.0);
    println!("  Files: {}", files.len());
    for path in files {
        println!("  {path}");
    }
    Ok(())
}

/// Read a single file from an archive and print to stdout or save to disk.
pub fn read_file_path(
    archive_path: &str,
    file_path: &str,
    output: Option<&str>,
    password: Option<&str>,
) -> anyhow::Result<()> {
    let pack = ResourcePack::open(archive_path, password)?;
    let data = pack.read_file(file_path)?;
    match output {
        Some(out_path) => {
            std::fs::write(out_path, &data)?;
            println!("{} bytes -> {}", data.len(), out_path);
        }
        None => {
            std::io::stdout().write_all(&data)?;
        }
    }
    Ok(())
}

/// Extract all files from an archive to a directory.
pub fn extract_all(
    archive_path: &str,
    output_dir: &str,
    password: Option<&str>,
) -> anyhow::Result<()> {
    use hexz_ops::pack::extract_archive;
    println!("Extracting {} -> {}", archive_path, output_dir);
    extract_archive(
        archive_path.as_ref(),
        output_dir.as_ref(),
        password.map(|s| s.to_string()),
    )
    .context("Failed to extract archive")?;
    println!("Done.");
    Ok(())
}

/// Show archive header metadata (compression, encryption, sizes).
pub fn show_metadata(
    archive_path: &str,
    json: bool,
    _password: Option<&str>,
) -> anyhow::Result<()> {
    use hexz_ops::inspect::inspect_archive;
    let info =
        inspect_archive(std::path::Path::new(archive_path)).context("Failed to inspect archive")?;
    let total = info.total_uncompressed();
    let ratio = info.compression_ratio();

    if json {
        let out = serde_json::json!({
            "path": archive_path,
            "version": info.version,
            "compression": format!("{:?}", info.compression),
            "block_size": info.block_size,
            "encrypted": info.features.encrypted,
            "original_size": total,
            "compressed_size": info.file_size,
            "compression_ratio": ratio,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        let comp = match info.compression {
            hexz_core::format::header::CompressionType::Lz4 => "LZ4",
            hexz_core::format::header::CompressionType::Zstd => "Zstd",
        };
        println!("Archive: {}", archive_path);
        println!("  version:     v{}", info.version);
        println!(
            "  compression: {} ({} KiB blocks)",
            comp,
            info.block_size / 1024
        );
        println!(
            "  size:        {} on disk, {} uncompressed ({:.2}x)",
            info.file_size, total, ratio
        );
        println!("  encrypted:   {}", info.features.encrypted);
    }
    Ok(())
}

/// Preview archive structure with file tree and metadata stats
pub fn preview_files(archive_path: &str, json: bool, password: Option<&str>) -> anyhow::Result<()> {
    let pack = ResourcePack::open(archive_path, password)?;
    let meta = pack.build_metadata();

    if json {
        println!("{}", serde_json::to_string_pretty(&meta)?);
    } else {
        use hexz_ops::inspect::inspect_archive;
        let info = inspect_archive(std::path::Path::new(archive_path))
            .context("Failed to inspect archive")?;

        println!("═══════════════════════════════════════");
        println!("  Archive: {}", archive_path);
        println!("  Version: v{}", info.version);
        let comp = match info.compression {
            hexz_core::format::header::CompressionType::Lz4 => "LZ4",
            hexz_core::format::header::CompressionType::Zstd => "Zstd",
        };
        println!(
            "  Compression: {} ({} KiB blocks)",
            comp,
            info.block_size / 1024
        );
        println!("  Encrypted: {}", info.features.encrypted);
        println!("  On-disk:  {}", hexz_k::format_size(info.file_size));
        println!("  Unpacked: {}", hexz_k::format_size(meta.total_size));
        println!("  Ratio: {:.2}x", info.compression_ratio());
        println!("  Files: {}", meta.total_files);

        println!("───────────────────────────────────────");
        println!("  Category breakdown:");
        let mut cats: Vec<_> = meta.category_counts.iter().collect();
        cats.sort_by_key(|(c, _)| format!("{c:?}"));
        for (cat, (count, size)) in &cats {
            let pct = if meta.total_size > 0 {
                *size as f64 / meta.total_size as f64 * 100.0
            } else {
                0.0
            };
            println!(
                "    {:<10} {:>5} files  {:>10}  ({:>4.0}%)",
                format!("{cat}:"),
                count,
                hexz_k::format_size(*size),
                pct,
            );
        }

        println!("───────────────────────────────────────");
        println!("  File tree:");
        print!("{}", meta.file_tree);
        println!("═══════════════════════════════════════");
    }
    Ok(())
}
