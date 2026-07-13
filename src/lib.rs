//! hexz_k library — embeddable hexz resource pack reader
//!
//! # Examples
//!
//! ```rust,no_run
//! use hexz_k::ResourcePack;
//! let pack = ResourcePack::open("game.hxz", None).unwrap();
//! let data = pack.read_file("bgm/bgm1.webm").unwrap();
//! let tree = pack.build_tree();
//! println!("{tree}");
//! ```

/// Shared benchmark data generators and measurement functions.
pub mod bench;

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::Path;
use std::sync::Arc;

mod archive {
    // re-export for internal use
    pub use hexz_core::ArchiveStream;
    pub use hexz_core::algo::encryption::{AesGcmEncryptor, Encryptor};
    pub use hexz_core::format::header::Header;
    pub use hexz_core::format::magic::HEADER_SIZE;
    pub use hexz_core::store::StorageBackend;
    pub use hexz_store;
}

/// A loaded hexz resource pack providing file-level random access.
///
/// Supports both encrypted and unencrypted archives.  Files can be
/// read individually by their path within the archive.
#[derive(Clone)]
pub struct ResourcePack {
    archive: Arc<hexz_core::Archive>,
    index: Arc<PackIndex>,
}

struct PackIndex {
    files: HashMap<String, FileEntry>,
    root_prefix: Option<String>,
}

#[derive(Clone)]
struct FileEntry {
    offset: u64,
    size: usize,
}

/// A resolved file handle for repeated or streaming reads.
///
/// Creating the handle performs the path lookup once. Cloning it is O(1), so
/// host engines can retain handles for voice, music, textures, and scripts.
#[derive(Clone)]
pub struct ResourceFile {
    archive: Arc<hexz_core::Archive>,
    offset: u64,
    size: usize,
}

impl ResourceFile {
    /// Uncompressed file length in bytes.
    pub const fn len(&self) -> usize {
        self.size
    }

    /// Return whether the file is empty.
    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Read the complete file into a newly allocated vector.
    pub fn read(&self) -> anyhow::Result<Vec<u8>> {
        self.archive
            .read_at(archive::ArchiveStream::Main, self.offset, self.size)
            .map_err(Into::into)
    }

    /// Read a range into a newly allocated vector.
    pub fn read_range(&self, offset: usize, length: usize) -> anyhow::Result<Vec<u8>> {
        if offset > self.size {
            anyhow::bail!("Range starts beyond end of resource file");
        }
        let length = length.min(self.size - offset);
        self.archive
            .read_at(
                archive::ArchiveStream::Main,
                self.checked_archive_offset(offset)?,
                length,
            )
            .map_err(Into::into)
    }

    /// Read from the start of the file into a reusable buffer.
    pub fn read_into(&self, buffer: &mut [u8]) -> anyhow::Result<usize> {
        self.read_range_into(0, buffer)
    }

    /// Read a range into a reusable buffer and return the bytes written.
    pub fn read_range_into(&self, offset: usize, buffer: &mut [u8]) -> anyhow::Result<usize> {
        if offset > self.size {
            anyhow::bail!("Range starts beyond end of resource file");
        }
        let length = buffer.len().min(self.size - offset);
        if length == 0 {
            return Ok(0);
        }
        self.archive.read_at_into(
            archive::ArchiveStream::Main,
            self.checked_archive_offset(offset)?,
            &mut buffer[..length],
        )?;
        Ok(length)
    }

    fn checked_archive_offset(&self, offset: usize) -> anyhow::Result<u64> {
        self.offset
            .checked_add(offset as u64)
            .ok_or_else(|| anyhow::anyhow!("Resource file offset overflow"))
    }
}

/// Runtime tuning for archive reads.
///
/// The default values preserve the historical behavior of [`ResourcePack::open`].
/// Game engines can select a bounded cache profile for predictable memory use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcePackOptions {
    /// Maximum number of decompressed blocks retained by the upstream cache.
    /// `None` uses the upstream default (currently 1000 blocks).
    pub cache_capacity_blocks: Option<usize>,
    /// Number of logical blocks to prefetch after a read. `None` disables prefetching.
    pub prefetch_window_blocks: Option<u32>,
    /// Verify an encrypted password by reading the first byte while opening.
    pub verify_password_on_open: bool,
}

impl Default for ResourcePackOptions {
    fn default() -> Self {
        Self {
            cache_capacity_blocks: None,
            prefetch_window_blocks: None,
            verify_password_on_open: true,
        }
    }
}

impl ResourcePackOptions {
    /// Memory-conscious profile for embedding in constrained host applications.
    ///
    /// At a typical 64 KiB block size, the decompressed block cache is bounded
    /// to roughly 16 MiB, excluding cache metadata and variable-size blocks.
    pub const fn memory_constrained() -> Self {
        Self {
            cache_capacity_blocks: Some(256),
            prefetch_window_blocks: None,
            verify_password_on_open: true,
        }
    }

    /// High-throughput profile for asset-heavy host applications.
    pub const fn high_throughput() -> Self {
        Self {
            cache_capacity_blocks: Some(1024),
            prefetch_window_blocks: None,
            verify_password_on_open: true,
        }
    }
}

/// Check whether an `.hxz` file is encrypted without fully opening it.
///
/// Reads only the header (first 512 bytes) via mmap.
pub fn is_encrypted(path: impl AsRef<Path>) -> anyhow::Result<bool> {
    use archive::hexz_store::local::MmapBackend;
    let backend: Arc<dyn archive::StorageBackend> = Arc::new(MmapBackend::new(path.as_ref())?);
    let bytes = backend.read_exact(0, archive::HEADER_SIZE)?;
    let header: archive::Header = bincode::deserialize(&bytes)?;
    Ok(header.encryption.is_some())
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

/// Human-readable file type classification based on extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum FileCategory {
    Image,
    Audio,
    Video,
    Script,
    Data,
    Text,
    Font,
    Archive,
    Unknown,
}

impl fmt::Display for FileCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileCategory::Image => write!(f, "Image"),
            FileCategory::Audio => write!(f, "Audio"),
            FileCategory::Video => write!(f, "Video"),
            FileCategory::Script => write!(f, "Script"),
            FileCategory::Data => write!(f, "Data"),
            FileCategory::Text => write!(f, "Text"),
            FileCategory::Font => write!(f, "Font"),
            FileCategory::Archive => write!(f, "Archive"),
            FileCategory::Unknown => write!(f, "Unknown"),
        }
    }
}

impl FileCategory {
    /// Classify a file by its extension.
    pub fn from_path(path: &str) -> Self {
        let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            // Images
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "tiff" | "avif" => {
                FileCategory::Image
            }
            // Audio
            "mp3" | "wav" | "ogg" | "flac" | "aac" | "m4a" | "wma" | "webm" | "opus" => {
                FileCategory::Audio
            }
            // Video
            "mp4" | "avi" | "mkv" | "mov" | "wmv" | "flv" | "m4v" => FileCategory::Video,
            // Scripts / code
            "js" | "ts" | "py" | "lua" | "rb" | "php" | "cs" | "go" | "rs" | "swift" | "kt"
            | "java" | "scala" | "r" | "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" => {
                FileCategory::Script
            }
            // Data formats
            "json" | "yaml" | "yml" | "toml" | "xml" | "csv" | "tsv" | "ini" | "cfg" | "conf"
            | "properties" | "env" | "plist" => FileCategory::Data,
            // Text
            "txt" | "md" | "rst" | "log" | "tex" | "css" | "html" | "htm" | "scss" | "sass"
            | "less" => FileCategory::Text,
            // Fonts
            "ttf" | "otf" | "woff" | "woff2" | "eot" => FileCategory::Font,
            // Archives
            "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "hxz" | "hxp" => {
                FileCategory::Archive
            }
            _ => FileCategory::Unknown,
        }
    }
}

/// A node in the file tree — either a directory or a file.
#[derive(Debug, Clone, Serialize)]
pub struct TreeNode {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub category: Option<FileCategory>,
    pub children: Vec<TreeNode>,
}

impl fmt::Display for TreeNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt_indent(f, "", true)
    }
}

impl TreeNode {
    fn fmt_indent(&self, f: &mut fmt::Formatter<'_>, prefix: &str, is_last: bool) -> fmt::Result {
        let connector = if is_last { "└── " } else { "├── " };
        if self.is_dir {
            writeln!(f, "{prefix}{connector}{}/", self.name)?;
        } else {
            let size_str = format_size(self.size.unwrap_or(0));
            let cat_str = self
                .category
                .as_ref()
                .map(|c| format!(" [{c}]"))
                .unwrap_or_default();
            writeln!(f, "{prefix}{connector}{}  ({size_str}){cat_str}", self.name)?;
        }
        let child_count = self.children.len();
        for (i, child) in self.children.iter().enumerate() {
            let child_prefix = if is_last {
                format!("{prefix}    ")
            } else {
                format!("{prefix}│   ")
            };
            child.fmt_indent(f, &child_prefix, i == child_count - 1)?;
        }
        Ok(())
    }
}

/// Format a byte count as a human-readable string (e.g. "2.3 MiB").
pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit_idx])
    }
}

/// Aggregate metadata for an archive: file count, total size,
/// per-category breakdowns, and the directory tree.
#[derive(Debug, Clone, Serialize)]
pub struct PackMetadata {
    pub total_files: usize,
    pub total_size: u64,
    pub category_counts: HashMap<FileCategory, (usize, u64)>,
    pub file_tree: TreeNode,
}

impl ResourcePack {
    /// Open a `.hxz` archive from the given path.
    ///
    /// If a password is provided, it will be used to decrypt the archive.
    /// Returns an error if the password is wrong or the archive is corrupted.
    pub fn open(path: impl AsRef<Path>, password: Option<&str>) -> anyhow::Result<Self> {
        Self::open_with_options(path, password, ResourcePackOptions::default())
    }

    /// Open an archive with explicit cache and prefetch tuning.
    pub fn open_with_options(
        path: impl AsRef<Path>,
        password: Option<&str>,
        options: ResourcePackOptions,
    ) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let archive = if let Some(pw) = password {
            let header = Self::read_header(path)?;
            if let Some(ref kp) = header.encryption {
                let enc: Box<dyn archive::Encryptor> = Box::new(
                    archive::AesGcmEncryptor::new(pw.as_bytes(), &kp.salt, kp.iterations)
                        .map_err(|e| anyhow::anyhow!("Key derivation failed: {e}"))?,
                );
                let a = Self::open_archive(path, Some(enc), options)
                    .map_err(|e| anyhow::anyhow!("Failed to open encrypted archive: {e}"))?;
                // Verify password on first block
                let size = a.size(archive::ArchiveStream::Main);
                if options.verify_password_on_open && size > 0 {
                    a.read_at(archive::ArchiveStream::Main, 0, 1)
                        .map_err(|_| anyhow::anyhow!("Wrong password or corrupted archive"))?;
                }
                a
            } else {
                Self::open_archive(path, None, options)?
            }
        } else {
            Self::open_archive(path, None, options)?
        };

        let index = Arc::new(Self::build_index(&archive)?);
        Ok(Self { archive, index })
    }

    /// Read a file by its path within the archive.
    ///
    /// Path separators are normalised to `/`.  Returns the raw bytes
    /// of the file, or an error if the file is not found.
    pub fn read_file(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let normalized = normalize_path(path);
        let entry = self
            .lookup_entry(&normalized)
            .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;

        self.archive
            .read_at(archive::ArchiveStream::Main, entry.offset, entry.size)
            .map_err(Into::into)
    }

    /// Return whether a path identifies a file in the archive.
    pub fn contains_file(&self, path: &str) -> bool {
        let normalized = normalize_path(path);
        self.lookup_entry(&normalized).is_some()
    }

    /// Resolve a path once for efficient repeated or streaming reads.
    pub fn open_file(&self, path: &str) -> anyhow::Result<ResourceFile> {
        let normalized = normalize_path(path);
        let entry = self
            .lookup_entry(&normalized)
            .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;
        Ok(ResourceFile {
            archive: Arc::clone(&self.archive),
            offset: entry.offset,
            size: entry.size,
        })
    }

    /// Return the uncompressed size of a file.
    pub fn file_size(&self, path: &str) -> Option<u64> {
        let normalized = normalize_path(path);
        self.lookup_entry(&normalized)
            .map(|entry| entry.size as u64)
    }

    /// Read a byte range from a file without materializing the entire file.
    pub fn read_file_range(
        &self,
        path: &str,
        offset: usize,
        length: usize,
    ) -> anyhow::Result<Vec<u8>> {
        let normalized = normalize_path(path);
        let entry = self
            .lookup_entry(&normalized)
            .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;
        if offset > entry.size {
            anyhow::bail!("Range starts beyond end of file: {path}");
        }
        let length = length.min(entry.size - offset);
        let archive_offset = entry
            .offset
            .checked_add(offset as u64)
            .ok_or_else(|| anyhow::anyhow!("File range overflow: {path}"))?;
        self.archive
            .read_at(archive::ArchiveStream::Main, archive_offset, length)
            .map_err(Into::into)
    }

    /// Read a file into a reusable caller-provided buffer.
    ///
    /// Returns the number of bytes written. If the buffer is larger than the
    /// file, its unused suffix is left unchanged.
    pub fn read_file_into(&self, path: &str, buffer: &mut [u8]) -> anyhow::Result<usize> {
        self.read_file_range_into(path, 0, buffer)
    }

    /// Read a file range into a reusable caller-provided buffer.
    ///
    /// Returns the number of bytes written. Reading at EOF returns zero.
    pub fn read_file_range_into(
        &self,
        path: &str,
        offset: usize,
        buffer: &mut [u8],
    ) -> anyhow::Result<usize> {
        let normalized = normalize_path(path);
        let entry = self
            .lookup_entry(&normalized)
            .ok_or_else(|| anyhow::anyhow!("File not found: {path}"))?;
        if offset > entry.size {
            anyhow::bail!("Range starts beyond end of file: {path}");
        }
        let length = buffer.len().min(entry.size - offset);
        if length == 0 {
            return Ok(0);
        }
        let archive_offset = entry
            .offset
            .checked_add(offset as u64)
            .ok_or_else(|| anyhow::anyhow!("File range overflow: {path}"))?;
        self.archive.read_at_into(
            archive::ArchiveStream::Main,
            archive_offset,
            &mut buffer[..length],
        )?;
        Ok(length)
    }

    /// List all file paths in the archive.
    pub fn list_files(&self) -> Vec<&str> {
        let mut files: Vec<_> = self.index.files.keys().map(String::as_str).collect();
        files.sort_unstable();
        files
    }

    /// Iterate over file paths without allocating a temporary list.
    pub fn iter_files(&self) -> impl Iterator<Item = &str> {
        self.index.files.keys().map(String::as_str)
    }

    /// Raw size of the main stream data (includes all blocks).
    pub fn main_size(&self) -> u64 {
        self.archive.size(archive::ArchiveStream::Main)
    }

    /// Number of blocks in the archive.
    pub fn block_count(&self) -> usize {
        self.archive
            .iter_block_hashes(archive::ArchiveStream::Main)
            .map(|h| h.len())
            .unwrap_or(0)
    }

    /// Build a directory tree from the file index.
    pub fn build_tree(&self) -> TreeNode {
        let mut root = TreeNode {
            name: "(root)".into(),
            is_dir: true,
            size: None,
            category: None,
            children: Vec::new(),
        };

        let mut children = BTreeMap::new();
        for (path, entry) in &self.index.files {
            Self::insert_tree_entry(
                &mut children,
                path.split('/').filter(|part| !part.is_empty()).peekable(),
                entry,
            );
        }
        root.children = children
            .into_iter()
            .map(|(name, node)| node.into_tree_node(name))
            .collect();

        // Remove root level if there's only one top-level dir
        if root.children.len() == 1 && root.children[0].is_dir {
            root = root.children.remove(0);
        }

        root
    }

    fn insert_tree_entry<'a, I>(
        children: &mut BTreeMap<String, TreeBuilderNode>,
        mut parts: std::iter::Peekable<I>,
        file: &FileEntry,
    ) where
        I: Iterator<Item = &'a str>,
    {
        let Some(name) = parts.next() else {
            return;
        };
        if parts.peek().is_none() {
            children.insert(name.to_owned(), TreeBuilderNode::File(file.clone()));
            return;
        }

        let node = children
            .entry(name.to_owned())
            .or_insert_with(|| TreeBuilderNode::Directory(BTreeMap::new()));
        if let TreeBuilderNode::Directory(next) = node {
            Self::insert_tree_entry(next, parts, file);
        }
    }

    /// Build full metadata summary including tree and category stats.
    pub fn build_metadata(&self) -> PackMetadata {
        let tree = self.build_tree();
        let mut category_counts: HashMap<FileCategory, (usize, u64)> = HashMap::new();
        let mut total_size: u64 = 0;

        for (path, file) in &self.index.files {
            let entry = category_counts
                .entry(FileCategory::from_path(path))
                .or_insert((0, 0));
            entry.0 += 1;
            entry.1 += file.size as u64;
            total_size += file.size as u64;
        }

        PackMetadata {
            total_files: self.index.files.len(),
            total_size,
            category_counts,
            file_tree: tree,
        }
    }

    fn read_header(path: &Path) -> anyhow::Result<archive::Header> {
        let backend: Arc<dyn archive::StorageBackend> =
            Arc::new(archive::hexz_store::local::MmapBackend::new(path)?);
        let bytes = backend.read_exact(0, archive::HEADER_SIZE)?;
        Ok(bincode::deserialize(&bytes)?)
    }

    fn open_archive(
        path: &Path,
        encryptor: Option<Box<dyn archive::Encryptor>>,
        options: ResourcePackOptions,
    ) -> anyhow::Result<Arc<hexz_core::Archive>> {
        archive::hexz_store::open_local_with_cache(
            path,
            encryptor,
            options.cache_capacity_blocks,
            options.prefetch_window_blocks,
        )
        .map_err(Into::into)
    }

    fn build_index(archive: &hexz_core::Archive) -> anyhow::Result<PackIndex> {
        let meta_bytes = archive
            .metadata
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No metadata in archive"))?;
        let meta: Metadata = serde_json::from_slice(meta_bytes)?;
        let mut index = HashMap::with_capacity(meta.files.len());
        for file in meta.files {
            let path = file.path.replace('\\', "/");
            let size = usize::try_from(file.size)
                .map_err(|_| anyhow::anyhow!("File is too large for this platform: {path}"))?;
            index.insert(
                path,
                FileEntry {
                    offset: file.offset,
                    size,
                },
            );
        }
        let root_prefix = common_root_prefix(index.keys().map(String::as_str));
        Ok(PackIndex {
            files: index,
            root_prefix,
        })
    }

    fn lookup_entry(&self, normalized: &str) -> Option<&FileEntry> {
        self.index.files.get(normalized).or_else(|| {
            let prefix = self.index.root_prefix.as_deref()?;
            let path = format!("{prefix}/{normalized}");
            self.index.files.get(&path)
        })
    }
}

enum TreeBuilderNode {
    Directory(BTreeMap<String, TreeBuilderNode>),
    File(FileEntry),
}

impl TreeBuilderNode {
    fn into_tree_node(self, name: String) -> TreeNode {
        match self {
            Self::Directory(children) => TreeNode {
                name,
                is_dir: true,
                size: None,
                category: None,
                children: children
                    .into_iter()
                    .map(|(name, node)| node.into_tree_node(name))
                    .collect(),
            },
            Self::File(file) => TreeNode {
                category: Some(FileCategory::from_path(&name)),
                name,
                is_dir: false,
                size: Some(file.size as u64),
                children: Vec::new(),
            },
        }
    }
}

fn normalize_path(path: &str) -> Cow<'_, str> {
    if path.contains('\\') {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path)
    }
}

fn common_root_prefix<'a>(mut paths: impl Iterator<Item = &'a str>) -> Option<String> {
    let first = paths.next()?;
    let (root, _) = first.split_once('/')?;
    if root.is_empty()
        || !paths.all(|path| path.split_once('/').is_some_and(|(head, _)| head == root))
    {
        return None;
    }
    Some(root.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn resource_handles_are_thread_safe_and_lightweight() {
        assert_send_sync::<ResourcePack>();
        assert_send_sync::<ResourceFile>();
        assert_eq!(
            std::mem::size_of::<ResourcePack>(),
            2 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn integration_profiles_have_bounded_caches() {
        let constrained = ResourcePackOptions::memory_constrained();
        let throughput = ResourcePackOptions::high_throughput();
        assert_eq!(constrained.cache_capacity_blocks, Some(256));
        assert_eq!(throughput.cache_capacity_blocks, Some(1024));
        assert!(constrained.prefetch_window_blocks.is_none());
        assert!(throughput.prefetch_window_blocks.is_none());
    }
}
