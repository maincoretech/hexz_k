# hexz_k

Game resource pack CLI & GUI tool based on the [hexz](https://github.com/hexz-org/hexz) archive format.

## Build

```bash
# CLI only (2.4 MB binary)
cargo build --release

# CLI + GUI (6.4 MB binary, requires egui/eframe)
cargo build --release --features gui
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
hexz bench                                           # 32 MiB, 3-round avg
```

## Benchmark

`hexz bench` generates 32 MiB of lorem-ipsum text, packs it in-process
with LZ4 and Zstd, then measures sequential read throughput and random
IOPS over 3 rounds.  Results reflect IO-bound performance; temp files
are cleaned up automatically.

Example output:

```text
  lz4 64KiB: pack    26ms   53.8 KiB  609.5x  |  seq  32003.9 MB/s  |  IOPS  1705
  zstd 64KiB: pack    20ms   45.0 KiB  727.6x  |  seq  32056.4 MB/s  |  IOPS  2632
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
