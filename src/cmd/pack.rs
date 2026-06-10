//! Pack command — uses hexz_ops::pack::pack_archive API

use anyhow::Context;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct PackOptions {
    pub input: String,
    pub output: String,
    pub compression: String,
    pub encrypt: bool,
    pub block_size: u32,
    pub password: Option<String>,
}

#[allow(dead_code)]
pub struct PackProgress {
    pub done: u64,
    pub total: u64,
    pub finished: bool,
}

#[derive(Default, Clone)]
pub struct ProgressTracker {
    pub inner: Arc<Mutex<(u64, u64, bool)>>,
}

impl ProgressTracker {
    pub fn new() -> Self { Self::default() }
    #[allow(dead_code)]
    pub fn get(&self) -> (u64, u64, bool) { *self.inner.lock().unwrap() }
}

pub fn pack_directory_with_progress(
    opts: &PackOptions,
    tracker: ProgressTracker,
) -> anyhow::Result<()> {
    use hexz_ops::pack::{pack_archive, PackConfig, PackTransformFlags};

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

pub fn pack_directory(opts: &PackOptions) -> anyhow::Result<()> {
    let tracker = ProgressTracker::new();
    pack_directory_with_progress(opts, tracker)
}
