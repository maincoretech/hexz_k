//! Pack command — uses `hexz_ops::pack::pack_archive`.

use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

struct StagedOutput {
    archive: PathBuf,
    requested: PathBuf,
    backup: Option<PathBuf>,
}

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

    let input = PathBuf::from(&opts.input);
    let requested_output = PathBuf::from(&opts.output);
    let staged_output = match staging_output_if_needed(&input, &requested_output) {
        Ok(output) => output,
        Err(error) => {
            tracker.inner.lock().unwrap().2 = true;
            return Err(error);
        }
    };
    let actual_output = staged_output
        .as_ref()
        .map_or(requested_output.as_path(), |staged| {
            staged.archive.as_path()
        });

    let config = PackConfig {
        input,
        output: actual_output.to_path_buf(),
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

    let mut result = pack_archive(&config, Some(&cb)).context("Failed to pack archive");
    if let Some(staged) = staged_output.as_ref() {
        if result.is_ok() {
            result = publish_staged_archive(&staged.archive, &staged.requested);
        }
        if result.is_ok() {
            if let Some(backup) = staged.backup.as_deref() {
                let _ = std::fs::remove_file(backup);
            }
        } else if let Err(restore_error) = restore_previous_archive(staged) {
            let original_error = result.unwrap_err();
            result = Err(anyhow::anyhow!(
                "{original_error:#}; additionally failed to restore previous archive: {restore_error:#}"
            ));
        }

        let _ = std::fs::remove_file(&staged.archive);
        if let Some(parent) = staged.archive.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
    tracker.inner.lock().unwrap().2 = true;
    result
}

/// Pack a directory without progress tracking (for CLI).
pub fn pack_directory(opts: &PackOptions) -> anyhow::Result<()> {
    let tracker = ProgressTracker::new();
    pack_directory_with_progress(opts, tracker)
}

fn staging_output_if_needed(input: &Path, output: &Path) -> anyhow::Result<Option<StagedOutput>> {
    if !input.is_dir() {
        return Ok(None);
    }

    let input = std::fs::canonicalize(input).context("Failed to resolve input directory")?;
    let output_parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let Ok(output_parent) = std::fs::canonicalize(output_parent) else {
        return Ok(None);
    };
    let requested = output_parent.join(output.file_name().unwrap_or_default());
    if !requested.starts_with(&input) {
        return Ok(None);
    }

    // hexz-ops deliberately skips every `.hexz` path component while walking
    // the input tree, so staging here prevents an in-tree output from packing
    // itself. The final archive is published only after packing completes.
    let staging_dir = input.join(".hexz");
    std::fs::create_dir_all(&staging_dir).context("Failed to create staging directory")?;
    let sequence = STAGING_COUNTER.fetch_add(1, Ordering::Relaxed);
    let archive = staging_dir.join(format!(
        "hexz-k-stage-{}-{sequence}.hxz",
        std::process::id()
    ));
    let backup = if requested.exists() {
        let backup = staging_dir.join(format!(
            "hexz-k-backup-{}-{sequence}.hxz",
            std::process::id()
        ));
        std::fs::rename(&requested, &backup)
            .context("Failed to stage the previous output archive")?;
        Some(backup)
    } else {
        None
    };
    Ok(Some(StagedOutput {
        archive,
        requested,
        backup,
    }))
}

fn publish_staged_archive(staged: &Path, output: &Path) -> anyhow::Result<()> {
    match std::fs::rename(staged, output) {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            // Windows cannot replace an existing destination with rename.
            // Copying preserves the CLI's existing overwrite behavior.
            std::fs::copy(staged, output).with_context(|| {
                format!("Failed to publish archive after rename failed ({rename_error})")
            })?;
            Ok(())
        }
    }
}

fn restore_previous_archive(staged: &StagedOutput) -> anyhow::Result<()> {
    if staged.requested.exists() {
        std::fs::remove_file(&staged.requested)
            .context("Failed to remove incomplete output archive")?;
    }
    if let Some(backup) = staged.backup.as_deref() {
        std::fs::rename(backup, &staged.requested)
            .context("Failed to restore previous output archive")?;
    }
    Ok(())
}
