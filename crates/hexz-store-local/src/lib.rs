//! Local-file subset of `hexz-store` for embedded game archives.
//!
//! Kēne never opens Hexz archives over HTTP or S3. This crate intentionally
//! implements only the API used by `hexz_k` and `hexz-ops` local packing, so
//! those paths do not pull an async runtime and two TLS stacks into the build.

use std::path::Path;
use std::sync::Arc;

use hexz_common::Result;
use hexz_core::algo::compression::create_compressor;
use hexz_core::algo::encryption::Encryptor;
use hexz_core::api::file::{Archive, ParentLoader};
use hexz_core::format::header::Header;

pub use hexz_core::store::StorageBackend;

pub mod local;

/// Open a local archive with the upstream cache defaults.
pub fn open_local(path: &Path, encryptor: Option<Box<dyn Encryptor>>) -> Result<Arc<Archive>> {
    open_local_with_cache(path, encryptor, None, None)
}

/// Open a local archive while preserving Hexz cache and parent-chain semantics.
pub fn open_local_with_cache(
    path: &Path,
    encryptor: Option<Box<dyn Encryptor>>,
    cache_capacity_blocks: Option<usize>,
    prefetch_window_blocks: Option<u32>,
) -> Result<Arc<Archive>> {
    let backend: Arc<dyn StorageBackend> = Arc::new(local::MmapBackend::new(path)?);
    let header = Header::read_from_backend(backend.as_ref())?;
    let dictionary = header.load_dictionary(backend.as_ref())?;
    let compressor = create_compressor(header.compression, None, dictionary.as_deref());
    let archive_dir = path.parent().unwrap_or_else(|| Path::new(".")).to_owned();
    let parent_loader: ParentLoader = Box::new(move |parent_path| {
        let parent_path = Path::new(parent_path);
        let resolved = if parent_path.exists() {
            parent_path.to_owned()
        } else {
            archive_dir.join(parent_path)
        };
        let backend: Arc<dyn StorageBackend> = Arc::new(local::MmapBackend::new(&resolved)?);
        Archive::open(backend, None)
    });

    Archive::with_cache_and_loader(
        backend,
        compressor,
        encryptor,
        cache_capacity_blocks,
        prefetch_window_blocks,
        Some(&parent_loader),
    )
}
