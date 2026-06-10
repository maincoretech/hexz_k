# hexz_k

Game resource pack tool based on [hexz](https://github.com/hexz-org/hexz) archive format.

## Build

```bash
# CLI only
cargo build --release

# With GUI
cargo build --release --features gui
```

## Usage

```bash
# Pack a directory (default: zstd, 64K blocks)
hexz pack ./game public game.hxz

# Pack with encryption
hexz pack ./game game.hxz -e

# Pack with LZ4 compression
hexz pack ./game game.hxz -c lz4

# List files
hexz list game.hxz

# Read a file to stdout
hexz read game.hxz bgm/bgm1.webm

# Extract all files
hexz extract game.hxz -o ./out

# Show metadata (human-readable)
hexz show game.hxz

# Show metadata (JSON)
hexz show game.hxz --json

# GUI mode (Keka-like)
hexz gui
```

## Commands

| Command | Description |
|---------|-------------|
| `hexz pack <in> <out> [-c lz4\|zstd] [-e] [--block-size N]` | Pack directory to .hxz |
| `hexz list <archive>` | List all files |
| `hexz read <archive> <path> [-o file]` | Read single file |
| `hexz extract <archive> [-o dir]` | Extract all files |
| `hexz show <archive> [--json]` | Show metadata |
| `hexz gui` | Launch GUI |

## License

MIT
