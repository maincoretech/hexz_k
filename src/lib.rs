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
use std::collections::HashMap;
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
    index: HashMap<String, (u64, usize)>,
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
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

    /// Read a file by its path within the archive.
    ///
    /// Path separators are normalised to `/`.  Returns the raw bytes
    /// of the file, or an error if the file is not found.
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

    /// List all file paths in the archive.
    pub fn list_files(&self) -> Vec<&str> {
        self.index.keys().map(|s| s.as_str()).collect()
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

        // Collect and sort paths
        let mut paths: Vec<&str> = self.index.keys().map(|s| s.as_str()).collect();
        paths.sort_unstable();

        for path in paths {
            let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
            if parts.is_empty() {
                continue;
            }
            Self::insert_into_tree(&mut root, &parts, self.index.get(path));
        }

        // Remove root level if there's only one top-level dir
        if root.children.len() == 1 && root.children[0].is_dir {
            root = root.children.remove(0);
        }

        root
    }

    fn insert_into_tree(node: &mut TreeNode, parts: &[&str], file_info: Option<&(u64, usize)>) {
        if parts.is_empty() {
            return;
        }

        let name = parts[0].to_string();

        if parts.len() == 1 {
            // It's a file
            let (_offset, size) = file_info.unwrap();
            let category = FileCategory::from_path(&name);
            node.children.push(TreeNode {
                name,
                is_dir: false,
                size: Some(*size as u64),
                category: Some(category),
                children: Vec::new(),
            });
        } else {
            // It's a directory
            let child = node
                .children
                .iter_mut()
                .find(|c| c.is_dir && c.name == name);
            if let Some(dir) = child {
                Self::insert_into_tree(dir, &parts[1..], file_info);
            } else {
                let mut new_dir = TreeNode {
                    name: name.clone(),
                    is_dir: true,
                    size: None,
                    category: None,
                    children: Vec::new(),
                };
                Self::insert_into_tree(&mut new_dir, &parts[1..], file_info);
                node.children.push(new_dir);
            }
        }
    }

    /// Build full metadata summary including tree and category stats.
    pub fn build_metadata(&self) -> PackMetadata {
        let tree = self.build_tree();
        let mut category_counts: HashMap<FileCategory, (usize, u64)> = HashMap::new();
        let mut total_size: u64 = 0;

        for (_path, (_offset, size)) in &self.index {
            let cat = FileCategory::from_path(_path);
            let entry = category_counts.entry(cat).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += *size as u64;
            total_size += *size as u64;
        }

        PackMetadata {
            total_files: self.index.len(),
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
