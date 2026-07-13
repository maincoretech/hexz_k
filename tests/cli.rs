//! Integration tests for hexz CLI commands.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn hexz_bin() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_hexz") {
        return PathBuf::from(p);
    }
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("target/{profile}/hexz"))
}

fn setup_temp_dir() -> PathBuf {
    let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("hexz_test_{id}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(dir.join("hello.txt"), b"Hello, hexz!\n").unwrap();
    std::fs::write(dir.join("data.bin"), [0u8; 1024]).unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/nested.txt"), b"nested file content\n").unwrap();

    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// ── Pack ──

#[test]
fn pack_and_list() {
    let dir = setup_temp_dir();
    let archive = dir.join("test.hxz");
    let bin = hexz_bin();
    assert!(bin.exists(), "hexz binary not found at {:?}", bin);

    // Pack
    let out = Command::new(&bin)
        .args(["pack", dir.to_str().unwrap(), archive.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "pack failed: {:?}", out);

    // Repacking to the same in-tree output must not include the previous archive.
    let out = Command::new(&bin)
        .args(["pack", dir.to_str().unwrap(), archive.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "repack failed: {:?}", out);

    let previous_archive = std::fs::read(&archive).unwrap();
    let out = Command::new(&bin)
        .args([
            "pack",
            dir.to_str().unwrap(),
            archive.to_str().unwrap(),
            "--compression",
            "invalid",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(
        std::fs::read(&archive).unwrap(),
        previous_archive,
        "failed repack must restore the previous archive"
    );

    // List
    let out = Command::new(&bin)
        .args(["list", archive.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("hello.txt"));
    assert!(stdout.contains("data.bin"));
    assert!(stdout.contains("sub/nested.txt"));
    assert!(
        !stdout.lines().any(|line| line.trim() == "test.hxz"),
        "archive must not include its own output"
    );

    cleanup(&dir);
}

#[test]
fn pack_with_compression_options() {
    let dir = setup_temp_dir();

    for comp in ["lz4", "zstd"] {
        let archive = dir.join(format!("test_{comp}.hxz"));
        let out = Command::new(hexz_bin())
            .args([
                "pack",
                dir.to_str().unwrap(),
                archive.to_str().unwrap(),
                "--compression",
                comp,
                "--block-size",
                "32768",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "pack {comp} failed: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(archive.exists());
        assert!(archive.metadata().unwrap().len() > 0);
    }

    cleanup(&dir);
}

// ── Read ──

#[test]
fn read_file() {
    let dir = setup_temp_dir();
    let archive = dir.join("test.hxz");
    let bin = hexz_bin();

    // Pack
    Command::new(&bin)
        .args(["pack", dir.to_str().unwrap(), archive.to_str().unwrap()])
        .output()
        .unwrap();

    let pack = hexz_k::ResourcePack::open(&archive, None).unwrap();
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<hexz_k::ResourcePack>();
    assert!(pack.contains_file("hello.txt"));
    assert!(!pack.contains_file("missing.txt"));
    assert_eq!(pack.file_size("hello.txt"), Some(13));
    assert_eq!(pack.read_file_range("hello.txt", 7, 4).unwrap(), b"hexz");

    let cloned = pack.clone();
    assert_eq!(cloned.read_file("hello.txt").unwrap(), b"Hello, hexz!\n");

    let file = pack.open_file("hello.txt").unwrap();
    assert_eq!(file.len(), 13);
    assert!(!file.is_empty());
    assert_eq!(file.read_range(7, 4).unwrap(), b"hexz");
    let cloned_file = file.clone();
    assert_eq!(cloned_file.read().unwrap(), b"Hello, hexz!\n");

    let mut buffer = [0xAA; 16];
    let read = pack.read_file_into("hello.txt", &mut buffer).unwrap();
    assert_eq!(read, 13);
    assert_eq!(&buffer[..read], b"Hello, hexz!\n");
    assert_eq!(&buffer[read..], &[0xAA; 3]);

    let mut range_buffer = [0u8; 4];
    let read = pack
        .read_file_range_into("hello.txt", 7, &mut range_buffer)
        .unwrap();
    assert_eq!(read, 4);
    assert_eq!(&range_buffer, b"hexz");

    let tuned_pack = hexz_k::ResourcePack::open_with_options(
        &archive,
        None,
        hexz_k::ResourcePackOptions::memory_constrained(),
    )
    .unwrap();
    assert_eq!(
        tuned_pack.read_file("hello.txt").unwrap(),
        b"Hello, hexz!\n"
    );

    // Read
    let out = Command::new(&bin)
        .args(["read", archive.to_str().unwrap(), "hello.txt"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(&out.stdout, b"Hello, hexz!\n");

    // Read to file
    let out_file = dir.join("extracted_hello.txt");
    Command::new(&bin)
        .args([
            "read",
            archive.to_str().unwrap(),
            "hello.txt",
            "--output",
            out_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(std::fs::read(&out_file).unwrap(), b"Hello, hexz!\n");

    cleanup(&dir);
}

#[test]
fn read_nonexistent_file() {
    let dir = setup_temp_dir();
    let archive = dir.join("test.hxz");
    let bin = hexz_bin();

    Command::new(&bin)
        .args(["pack", dir.to_str().unwrap(), archive.to_str().unwrap()])
        .output()
        .unwrap();

    let out = Command::new(&bin)
        .args(["read", archive.to_str().unwrap(), "nonexistent.txt"])
        .output()
        .unwrap();
    assert!(!out.status.success());

    cleanup(&dir);
}

// ── Extract ──

#[test]
fn extract_all() {
    let dir = setup_temp_dir();
    let archive = dir.join("test.hxz");
    let bin = hexz_bin();

    Command::new(&bin)
        .args(["pack", dir.to_str().unwrap(), archive.to_str().unwrap()])
        .output()
        .unwrap();

    let out_dir = dir.join("extracted");
    let out = Command::new(&bin)
        .args([
            "extract",
            archive.to_str().unwrap(),
            "--output",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success());

    assert!(out_dir.join("hello.txt").exists());
    assert!(out_dir.join("data.bin").exists());
    assert!(out_dir.join("sub/nested.txt").exists());
    assert_eq!(
        std::fs::read(out_dir.join("hello.txt")).unwrap(),
        b"Hello, hexz!\n"
    );

    cleanup(&dir);
}

// ── Show ──

#[test]
fn show_metadata() {
    let dir = setup_temp_dir();
    let archive = dir.join("test.hxz");
    let bin = hexz_bin();

    Command::new(&bin)
        .args(["pack", dir.to_str().unwrap(), archive.to_str().unwrap()])
        .output()
        .unwrap();

    let out = Command::new(&bin)
        .args(["show", archive.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("version"));
    assert!(stdout.contains("compression"));

    // JSON mode
    let out = Command::new(&bin)
        .args(["show", archive.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(json["path"], archive.to_str().unwrap());

    cleanup(&dir);
}

// ── Preview ──

#[test]
fn preview() {
    let dir = setup_temp_dir();
    let archive = dir.join("test.hxz");
    let bin = hexz_bin();

    Command::new(&bin)
        .args(["pack", dir.to_str().unwrap(), archive.to_str().unwrap()])
        .output()
        .unwrap();

    let out = Command::new(&bin)
        .args(["preview", archive.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello.txt"));
    assert!(stdout.contains("data.bin"));
    assert!(stdout.contains("Files:"));

    // JSON mode
    let out = Command::new(&bin)
        .args(["preview", archive.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    serde_json::from_slice::<serde_json::Value>(&out.stdout).expect("valid JSON");

    cleanup(&dir);
}

// ── Bench (slow — run with --ignored) ──

#[test]
#[ignore = "slow: runs full benchmark"]
fn bench_runs() {
    let out = Command::new(hexz_bin()).arg("bench").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hexz_k Benchmark"));
    assert!(stdout.contains("lz4"));
    assert!(stdout.contains("zstd"));
    assert!(stdout.contains("MB/s"));
    assert!(stdout.contains("IOPS"));
}

// ── Help ──

#[test]
fn help_shows_commands() {
    let out = Command::new(hexz_bin()).arg("--help").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    for cmd in [
        "pack", "list", "read", "extract", "show", "preview", "bench", "gui",
    ] {
        assert!(stdout.contains(cmd), "help missing command: {cmd}");
    }
}

// ── Encrypted pack (requires --ignored, may prompt for password) ──

#[test]
#[ignore = "interactive: prompts for password"]
fn pack_encrypted() {
    let dir = setup_temp_dir();
    let archive = dir.join("encrypted.hxz");
    let bin = hexz_bin();

    let out = Command::new(&bin)
        .args([
            "pack",
            dir.to_str().unwrap(),
            archive.to_str().unwrap(),
            "--encrypt",
        ])
        .env("HEXZ_PASSWORD", "testpass123")
        .output()
        .unwrap();
    assert!(out.status.success());

    // List without password should fail
    let out = Command::new(&bin)
        .args(["list", archive.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());

    // List with password should succeed
    let out = Command::new(&bin)
        .args(["list", archive.to_str().unwrap()])
        .env("HEXZ_PASSWORD", "testpass123")
        .output()
        .unwrap();
    assert!(out.status.success());

    cleanup(&dir);
}
