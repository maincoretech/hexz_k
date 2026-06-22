//! Pack command — uses `hexz_ops::pack::pack_archive`.

use anyhow::Context;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Configuration for packing a directory into a `.hxz` archive.
pub struct PackOptions {
    /// Source directory path.
    pub input: String,
    /// Output `.hxz` file path.
    pub output: String,
    /// Compression algorithm: `"lz4"` or `"zstd"`.
    pub compression: String,
    /// Enable AES-256-GCM encryption.
    pub encrypt: bool,
    /// Block size in bytes (default 65536).
    pub block_size: u32,
    /// Optional encryption password.
    pub password: Option<String>,
}

/// Thread-safe progress tracker for pack operations.
#[derive(Default, Clone)]
pub struct ProgressTracker {
    pub inner: Arc<Mutex<(u64, u64, bool)>>,
}

impl ProgressTracker {
    /// Create a new progress tracker with zero progress.
    pub fn new() -> Self {
        Self::default()
    }
    /// Read current progress: (done, total, finished).
    #[cfg(feature = "gui")]
    pub fn get(&self) -> (u64, u64, bool) {
        *self.inner.lock().unwrap()
    }
}

/// Pack a directory with progress callback (for GUI).
pub fn pack_directory_with_progress(
    opts: &PackOptions,
    tracker: ProgressTracker,
) -> anyhow::Result<()> {
    use hexz_ops::pack::{PackConfig, PackTransformFlags, pack_archive};

    let t = tracker.clone();
    let cb = move |done: u64, total: u64| {
        let mut p = t.inner.lock().unwrap();
        p.0 = done;
        p.1 = total;
    };

    let config = PackConfig {
        input: PathBuf::from(&opts.input),
        output: PathBuf::from(&opts.output),
        compression: opts.compression.clone(),
        password: opts.password.clone(),
        num_workers: 0, // 0 = auto-detect CPU cores
        transform: PackTransformFlags {
            encrypt: opts.encrypt,
            train_dict: false,
            parallel: true, // enable multi-threaded compression
        },
        block_size: opts.block_size,
        ..Default::default()
    };

    pack_archive(&config, Some(&cb)).context("Failed to pack archive")?;

    tracker.inner.lock().unwrap().2 = true;
    Ok(())
}

/// Pack a directory without progress tracking (for CLI).
pub fn pack_directory(opts: &PackOptions) -> anyhow::Result<()> {
    let tracker = ProgressTracker::new();
    pack_directory_with_progress(opts, tracker)
}
