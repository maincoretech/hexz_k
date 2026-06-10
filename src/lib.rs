//! hexz_k library — embeddable hexz resource pack reader
//!
//! ```rust,no_run
//! use hexz_k::ResourcePack;
//! let pack = ResourcePack::open("game.hxz", None).unwrap();
//! let data = pack.read_file("bgm/bgm1.webm").unwrap();
//! ```

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

mod archive {
    // re-export for internal use
    pub use hexz_core::algo::encryption::{AesGcmEncryptor, Encryptor};
    pub use hexz_core::format::header::Header;
    pub use hexz_core::format::magic::HEADER_SIZE;
    pub use hexz_core::store::StorageBackend;
    pub use hexz_core::ArchiveStream;
    pub use hexz_store;
}

/// A loaded hexz resource pack with file-level random access
pub struct ResourcePack {
    archive: Arc<hexz_core::Archive>,
    index: HashMap<String, (u64, usize)>,
}

#[derive(Debug, Deserialize)]
struct MetaFile {
    path: String,
    offset: u64,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    files: Vec<MetaFile>,
}

impl ResourcePack {
    /// Open a .hxz archive, optionally with encryption password
    pub fn open(path: impl AsRef<Path>, password: Option<&str>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let archive = if let Some(pw) = password {
            let header = Self::read_header(path)?;
            if let Some(ref kp) = header.encryption {
                let enc: Box<dyn archive::Encryptor> = Box::new(
                    archive::AesGcmEncryptor::new(pw.as_bytes(), &kp.salt, kp.iterations)
                        .map_err(|e| anyhow::anyhow!("Key derivation failed: {e}"))?,
                );
                let a = archive::hexz_store::open_local(path, Some(enc))
                    .map_err(|e| anyhow::anyhow!("Failed to open encrypted archive: {e}"))?;
                // Verify password on first block
                let size = a.size(archive::ArchiveStream::Main);
                if size > 0 {
                    a.read_at(archive::ArchiveStream::Main, 0, 1)
                        .map_err(|_| anyhow::anyhow!("Wrong password or corrupted archive"))?;
                }
                a
            } else {
                archive::hexz_store::open_local(path, None::<Box<dyn archive::Encryptor>>)?
            }
        } else {
            archive::hexz_store::open_local(path, None::<Box<dyn archive::Encryptor>>)?
        };

        let index = Self::build_index(&archive)?;
        Ok(Self { archive, index })
    }

    /// Read a file by its path within the archive
    pub fn read_file(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let normalized = path.replace('\\', "/");
        let (offset, size) = self
            .index
            .get(&normalized)
            .or_else(|| {
                self.index
                    .iter()
                    .find(|(k, _)| k.ends_with(&normalized))
                    .map(|(_, v)| v)
            })
            .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;

        self.archive
            .read_at(archive::ArchiveStream::Main, *offset, *size)
            .map_err(Into::into)
    }

    /// List all file paths in the archive
    pub fn list_files(&self) -> Vec<&str> {
        self.index.keys().map(|s| s.as_str()).collect()
    }

    /// Get total uncompressed size of the main stream
    pub fn total_size(&self) -> u64 {
        self.archive.size(archive::ArchiveStream::Main)
    }

    fn read_header(path: &Path) -> anyhow::Result<archive::Header> {
        let backend: Arc<dyn archive::StorageBackend> =
            Arc::new(archive::hexz_store::local::MmapBackend::new(path)?);
        let bytes = backend.read_exact(0, archive::HEADER_SIZE)?;
        Ok(bincode::deserialize(&bytes)?)
    }

    fn build_index(archive: &hexz_core::Archive) -> anyhow::Result<HashMap<String, (u64, usize)>> {
        let meta_bytes = archive
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No metadata in archive"))?;
        let meta: Metadata = serde_json::from_slice(meta_bytes)?;
        let mut idx = HashMap::new();
        for f in &meta.files {
            idx.insert(f.path.replace('\\', "/"), (f.offset, f.size as usize));
        }
        Ok(idx)
    }
}
