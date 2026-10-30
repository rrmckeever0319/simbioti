mod watch;

use clap::{Parser, Subcommand};
use photosync_core::{run_client, Config, HubServer};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "photosync",
    about = "Sync photos and videos over an encrypted Noise channel"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print a new 32-byte pre-shared key as hex.
    Keygen,
    /// Listen for a client and exchange missing media.
    Hub {
        #[arg(long)]
        config: PathBuf,
        /// Exit after one session.
        #[arg(long)]
        once: bool,
    },
    /// Connect to a hub and exchange missing media.
    Client {
        #[arg(long)]
        config: PathBuf,
        /// Sync once, then sync again when media in the folder changes.
        #[arg(long)]
        watch: bool,
    },
}

fn main() -> ExitCode {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    init_log();
    match Cli::parse().cmd {
        Cmd::Keygen => keygen()?,
        Cmd::Hub { config, once } => {
            let cfg = Config::load(&config)?;
            let hub = HubServer::bind(&cfg)?;
            println!("PHOTOSYNC_LISTEN {}", hub.local_addr()?);
            std::io::stdout().flush()?;
            let report = hub.serve(once)?;
            println!("sent {} received {}", report.sent, report.received);
        }
        Cmd::Client { config, watch } => {
            let cfg = Config::load(&config)?;
            if watch {
                watch::watch(cfg)?;
            } else {
                let report = run_client(&cfg)?;
                println!("sent {} received {}", report.sent, report.received);
            }
        }
    }
    Ok(())
}

fn init_log() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

fn keygen() -> std::io::Result<()> {
    let mut key = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut key)?;
    println!("{}", hex::encode(key));
    Ok(())
}
