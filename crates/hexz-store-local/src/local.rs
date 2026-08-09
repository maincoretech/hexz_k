//! Read-only memory-mapped Hexz storage.

use std::fs::File;
use std::path::Path;

use bytes::Bytes;
use hexz_common::Result;
use hexz_core::store::StorageBackend;
use memmap2::Mmap;

#[derive(Debug)]
pub struct MmapBackend {
    bytes: Bytes,
}

impl MmapBackend {
    pub fn new(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        // SAFETY: Hexz packages are immutable while mounted. The mapping owns
        // its file-backed pages through `Bytes::from_owner` and is read-only.
        let mapping = unsafe { Mmap::map(&file)? };
        Ok(Self {
            bytes: Bytes::from_owner(mapping),
        })
    }
}

impl StorageBackend for MmapBackend {
    fn read_exact(&self, offset: u64, len: usize) -> Result<Bytes> {
        let start = usize::try_from(offset).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "archive offset exceeds usize",
            )
        })?;
        let end = start.checked_add(len).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "archive range overflow")
        })?;
        let bytes = self.bytes.get(start..end).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "archive read exceeds mapped file",
            )
        })?;
        Ok(self.bytes.slice_ref(bytes))
    }

    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
}
