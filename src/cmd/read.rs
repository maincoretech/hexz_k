//! Read/list/extract/show — use hexz_ops APIs (aligned with original hexz-cli)

use anyhow::Context;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::Write;

#[derive(Debug, Deserialize)]
struct HexzMetaFile { path: String, offset: u64, size: u64 }

#[derive(Debug, Deserialize)]
struct HexzMetadata { files: Vec<HexzMetaFile> }

fn open_archive(path: &std::path::Path, password: Option<&str>) -> anyhow::Result<std::sync::Arc<hexz_core::Archive>> {
    if let Some(pw) = password {
        let header = read_header(path)?;
        if let Some(ref kp) = header.encryption {
            use hexz_core::algo::encryption::AesGcmEncryptor;
            let encryptor: Box<dyn hexz_core::algo::encryption::Encryptor> =
                Box::new(AesGcmEncryptor::new(pw.as_bytes(), &kp.salt, kp.iterations)
                    .map_err(|e| anyhow::anyhow!("Encryption error: {e}"))?);
            let archive = hexz_store::open_local(path, Some(encryptor))
                .context("Failed to open encrypted archive")?;
            // Verify password by reading first byte — triggers GCM auth tag check on first block
            let size = archive.size(hexz_core::ArchiveStream::Main);
            if size > 0 {
                archive.read_at(hexz_core::ArchiveStream::Main, 0, 1)
                    .map_err(|_| anyhow::anyhow!("Wrong password or corrupted archive"))?;
            }
            Ok(archive)
        } else {
            hexz_store::open_local(path, None::<Box<dyn hexz_core::algo::encryption::Encryptor>>)
                .context("Failed to open archive")
        }
    } else {
        hexz_store::open_local(path, None::<Box<dyn hexz_core::algo::encryption::Encryptor>>)
            .context("Failed to open archive (maybe encrypted?)")
    }
}

fn read_header(path: &std::path::Path) -> anyhow::Result<hexz_core::format::header::Header> {
    use hexz_core::format::header::Header;
    let backend: std::sync::Arc<dyn hexz_core::store::StorageBackend> =
        std::sync::Arc::new(hexz_store::local::MmapBackend::new(path)?);
    let header_bytes = backend.read_exact(0, hexz_core::format::magic::HEADER_SIZE)?;
    let header: Header = bincode::deserialize(&header_bytes)?;
    Ok(header)
}

fn build_file_index(archive: &hexz_core::Archive) -> anyhow::Result<HashMap<String, (u64, usize)>> {
    let metadata = archive.metadata.as_ref().context("No metadata in archive")?;
    let meta: HexzMetadata = serde_json::from_str(
        std::str::from_utf8(metadata).context("bad utf8")?
    )?;
    let mut index = HashMap::new();
    for f in &meta.files {
        index.insert(f.path.replace('\\', "/"), (f.offset, f.size as usize));
    }
    Ok(index)
}

pub fn list_files(archive_path: &str, password: Option<&str>) -> anyhow::Result<()> {
    let path = std::path::Path::new(archive_path);
    let archive = open_archive(path, password)?;
    let main_size = archive.size(hexz_core::ArchiveStream::Main);
    let hashes = archive.iter_block_hashes(hexz_core::ArchiveStream::Main)
        .context("Failed to iterate blocks")?;
    println!("Archive: {}", archive_path);
    println!("Size: {:.2} MB  Blocks: {}", main_size as f64 / 1_048_576.0, hashes.len());
    match build_file_index(&archive) {
        Ok(index) => {
            let mut files: Vec<_> = index.iter().collect();
            files.sort_by_key(|(p, _)| *p);
            println!("Files: {}", files.len());
            for (path, (offset, size)) in &files {
                println!("  {:>10} B  @{:<10}  {}", size, offset, path);
            }
        }
        Err(e) => println!("No file index: {e}"),
    }
    Ok(())
}

pub fn read_file_path(archive_path: &str, file_path: &str, output: Option<&str>, password: Option<&str>) -> anyhow::Result<()> {
    let path = std::path::Path::new(archive_path);
    let archive = open_archive(path, password)?;
    let index = build_file_index(&archive)?;
    let normalized = file_path.replace('\\', "/");
    let (offset, size) = index.get(&normalized)
        .or_else(|| index.iter().find(|(k, _)| k.ends_with(&normalized)).map(|(_, v)| v))
        .context(format!("File not found: {}", file_path))?;
    let data = archive.read_at(hexz_core::ArchiveStream::Main, *offset, *size)?;
    match output {
        Some(out_path) => { fs::write(out_path, &data)?; println!("{} bytes -> {}", data.len(), out_path); }
        None => { std::io::stdout().write_all(&data)?; }
    }
    Ok(())
}

pub fn extract_all(archive_path: &str, output_dir: &str, password: Option<&str>) -> anyhow::Result<()> {
    use hexz_ops::pack::extract_archive;
    println!("Extracting {} -> {}", archive_path, output_dir);
    extract_archive(archive_path.as_ref(), output_dir.as_ref(), password.map(|s| s.to_string()))
        .context("Failed to extract archive")?;
    println!("Done.");
    Ok(())
}

pub fn show_metadata(archive_path: &str, json: bool, _password: Option<&str>) -> anyhow::Result<()> {
    use hexz_ops::inspect::inspect_archive;
    let info = inspect_archive(std::path::Path::new(archive_path))
        .context("Failed to inspect archive")?;
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
        println!("  compression: {} ({} KiB blocks)", comp, info.block_size / 1024);
        println!("  size:        {} on disk, {} uncompressed ({:.2}x)",
            info.file_size, total, ratio);
        println!("  encrypted:   {}", info.features.encrypted);
    }
    Ok(())
}
