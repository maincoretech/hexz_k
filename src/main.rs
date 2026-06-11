//! hexz_k — hexz archive tool for game resource packs
//!
//! Usage:
//!   hexz pack <input_dir> <output.hxz>   — pack a directory
//!   hexz gui                              — launch GUI mode

use clap::{Parser, Subcommand};

mod cmd;

#[derive(Parser)]
#[command(name = "hexz", about = "hexz archive CLI for game resource packs")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Pack a directory into a .hxz archive
    Pack {
        /// Input directory path
        input: String,
        /// Output .hxz file path
        output: String,
        /// Compression algorithm: "lz4" (fast) or "zstd" (high ratio)
        #[arg(short, long, default_value = "zstd")]
        compression: String,
        /// Enable AES-256-GCM encryption
        #[arg(short, long)]
        encrypt: bool,
        /// Block size in bytes (default: 65536)
        #[arg(long, default_value_t = 65536)]
        block_size: u32,
    },
    /// List files in a .hxz archive
    List {
        archive: String,
    },
    /// Read and output a file from a .hxz archive
    Read {
        archive: String,
        file: String,
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Extract all files from a .hxz archive
    Extract {
        archive: String,
        #[arg(short, long, default_value = "extracted")]
        output: String,
    },
    /// Show detailed archive metadata
    Show {
        archive: String,
        #[arg(long)]
        json: bool,
    },
    /// Launch GUI mode (Keka-like archive tool)
    Gui,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None | Some(Command::Gui) => {
            #[cfg(feature = "gui")]
            {
                return cmd::gui::run_gui();
            }
            #[cfg(not(feature = "gui"))]
            {
                println!("GUI mode not compiled. Rebuild with: cargo build --features gui");
                println!("Or use CLI: hexz pack <input> <output>");
            }
        }
        Some(Command::Pack { input, output, compression, encrypt, block_size }) => {
            let password = if encrypt {
                Some(match std::env::var("HEXZ_PASSWORD") {
                    Ok(p) => p,
                    Err(_) => rpassword::prompt_password("Encryption password: ")?,
                })
            } else { None };
            cmd::pack::pack_directory(&cmd::pack::PackOptions {
                input, output, compression, encrypt, block_size, password,
            })?;
        }
        Some(Command::List { archive }) => {
            let password = read_password_if_needed(&archive)?;
            cmd::read::list_files(&archive, password.as_deref())?;
        }
        Some(Command::Read { archive, file, output }) => {
            let password = read_password_if_needed(&archive)?;
            cmd::read::read_file_path(&archive, &file, output.as_deref(), password.as_deref())?;
        }
        Some(Command::Extract { archive, output }) => {
            let password = read_password_if_needed(&archive)?;
            cmd::read::extract_all(&archive, &output, password.as_deref())?;
        }
        Some(Command::Show { archive, json }) => {
            let password = read_password_if_needed(&archive)?;
            cmd::read::show_metadata(&archive, json, password.as_deref())?;
        }
    }
    Ok(())
}

fn read_password_if_needed(archive_path: &str) -> anyhow::Result<Option<String>> {
    use std::io::Read;
    let mut file = std::fs::File::open(archive_path)?;
    let mut header_bytes = [0u8; 512];
    file.read_exact(&mut header_bytes)?;
    if let Ok(header) = bincode::deserialize::<hexz_core::format::header::Header>(&header_bytes) {
        if header.encryption.is_some() {
            return Ok(Some(match std::env::var("HEXZ_PASSWORD") {
                Ok(p) => p,
                Err(_) => rpassword::prompt_password("Decryption password: ")?,
            }));
        }
    }
    Ok(None)
}
