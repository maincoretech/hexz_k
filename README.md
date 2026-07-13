# hexz_k

Game resource pack CLI & GUI tool based on the [hexz](https://github.com/hexz-org/hexz) archive format.

## Build

```bash
# CLI only (2.4 MB binary)
cargo build --release

# CLI + GUI (6.4 MB binary, requires egui/eframe)
cargo build --release --features gui
```

When embedding the resource backend into another project, depend on the library
without CLI/GUI dependencies:

```toml
hexz_k = { version = "0.2", default-features = false }
```

```rust,no_run
use hexz_k::{ResourcePack, ResourcePackOptions};

let pack = ResourcePack::open_with_options(
    "game.hxz",
    None,
    ResourcePackOptions::memory_constrained(),
)?;
let file = pack.open_file("scenario/scene-0001.txt")?;
let mut buffer = vec![0; file.len()];
let bytes_read = file.read_into(&mut buffer)?;
# Ok::<(), anyhow::Error>(())
```

## Commands

| Command | Description |
|---------|-------------|
| `pack <in> <out> [-c lz4\|zstd] [-e] [--block-size N]` | Pack a directory into .hxz |
| `list <archive>` | List all files in archive |
| `read <archive> <path> [-o file]` | Read a single file |
| `extract <archive> [-o dir]` | Extract all files to directory |
| `show <archive> [--json]` | Show archive metadata |
| `preview <archive> [--json]` | File tree, categories, compression stats |
| `bench` | Run compression & IO benchmark |
| `gui` | Launch GUI archive manager |

## Usage

```bash
# ── Pack ──
hexz pack ./assets game.hxz                          # zstd (default)
hexz pack ./assets game.hxz --compression lz4        # LZ4 (faster)
hexz pack ./assets game.hxz --encrypt                # AES-256-GCM

# ── Inspect ──
hexz list game.hxz                                   # list all files
hexz show game.hxz                                   # human-readable metadata
hexz show game.hxz --json                            # JSON output
hexz preview game.hxz                                # file tree + categories

# ── Read / Extract ──
hexz read game.hxz bgm/bgm1.webm                     # stdout
hexz read game.hxz bgm/bgm1.webm --output bgm1.webm  # save to file
hexz extract game.hxz --output ./out                 # extract all

# ── Benchmark ──
hexz bench                                           # 28 MiB mixed assets, 3-round avg
```

## Benchmark

`hexz bench` generates a 28 MiB visual-novel/game workload containing 624
scenario, background, voice, and UI assets. It packs them in-process with
LZ4 and Zstd, then measures sequential throughput and allocation-free random
4 KiB range-read IOPS over 3 rounds. Input data and benchmark archives use
separate directories, and temp files are cleaned up automatically.

Example output:

```text
  lz4 64KiB: pack   180ms  24999.6 KiB   1.1x  |  seq  1325.1 MB/s  |  IOPS  640016
  zstd 64KiB: pack   180ms  24881.9 KiB   1.2x  |  seq  1252.9 MB/s  |  IOPS  459199
```

## Architecture

```
src/
├── lib.rs          Library: ResourcePack, FileCategory, TreeNode, format_size, is_encrypted
├── bench.rs        Shared bench helpers: lorem, BENCH_CONFIGS, generate_test_files, measure_reads
├── main.rs         CLI entry point (clap)
└── cmd/
    ├── mod.rs      Module declarations
    ├── pack.rs     Pack command (hexz_ops wrapper)
    ├── read.rs     List / Read / Extract / Show / Preview commands
    ├── bench.rs    CLI benchmark runner
    └── gui.rs      egui GUI (feature-gated)
```

## License

MIT
