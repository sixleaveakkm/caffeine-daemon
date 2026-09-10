use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use clap::{Parser, Subcommand};

use caffeine_daemon::caffeinate::Inhibitor;
use caffeine_daemon::config::Config;
use caffeine_daemon::holds::{Holds, SystemClock};
use caffeine_daemon::hook;
use caffeine_daemon::launchagent;
use caffeine_daemon::lock::Lock;
use caffeine_daemon::server::Api;
use caffeine_daemon::{client, server};

/// Floor for the sweep interval, so a tiny `ttl` cannot spin the thread.
const MIN_SWEEP: Duration = Duration::from_secs(1);
/// Only reached when `bind` has no parseable port, which the server rejects anyway.
const DEFAULT_PORT: u16 = 8787;

/// Configuration is not passed here: it comes from the config file, or the
/// built-in defaults.
#[derive(Debug, Parser)]
#[command(
    name = "caffeine-daemon",
    version,
    about = "Keeps a Mac awake while any caller holds it"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show what a running daemon is doing
    Status {
        /// Print without ANSI colour
        #[arg(long)]
        no_color: bool,
    },
    /// End a hold by id, without waiting for its ttl
    Stop {
        /// The hold's id, as shown by `status`
        id: String,
    },
    /// Print the Claude Code hook script, to save somewhere and run
    PrintScript,
    /// Write the LaunchAgent plist that runs this daemon at login
    InstallLaunchagent {
        /// Replace an existing plist that has been changed by hand
        #[arg(long)]
        force: bool,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        None => serve(),
        Some(Command::Status { no_color }) => status(no_color),
        Some(Command::Stop { id }) => stop(&id),
        Some(Command::PrintScript) => print_script(),
        Some(Command::InstallLaunchagent { force }) => install_launchagent(force),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("caffeine-daemon: {err}");
            ExitCode::FAILURE
        }
    }
}

type Failure = Box<dyn std::error::Error>;

fn serve() -> Result<(), Failure> {
    let config = Config::load()?;
    // Held until the process exits: two daemons would each keep their own
    // caffeinate, and only one of them would answer on the configured port.
    let _lock = Lock::acquire()?;
    let holds = Arc::new(Holds::new(config.ttl, inhibitor(), Arc::new(SystemClock)));

    // Sweeping once per ttl is enough to bound how long a dead session keeps
    // the Mac awake at 2 x ttl, and costs one wake-up per ttl to do it.
    let interval = config.ttl.max(MIN_SWEEP);
    let sweeper = holds.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(interval);
            if let Err(err) = sweeper.sweep() {
                eprintln!("caffeine-daemon: sweep failed: {err}");
            }
        }
    });

    server::serve(
        Api {
            holds,
            api_key: config.api_key,
        },
        &config.bind,
    )
}

fn status(no_color: bool) -> Result<(), Failure> {
    let config = Config::load()?;
    let status = client::fetch(&config.bind)?;
    print!("{}", client::render(&status, client::use_color(no_color)));
    Ok(())
}

fn stop(id: &str) -> Result<(), Failure> {
    let config = Config::load()?;
    let held = client::fetch(&config.bind)?
        .holds
        .into_iter()
        .find(|hold| hold.id == id)
        .ok_or_else(|| format!("no live hold with id `{id}`"))?;

    client::release(&config.bind, id, config.api_key.as_deref())?;
    println!("released {} ({})", held.id, held.title);
    Ok(())
}

/// The printed script defaults to the configured port and key, and still lets
/// CAFFEINE_PORT and CAFFEINE_API_KEY override them.
fn print_script() -> Result<(), Failure> {
    let config = Config::load()?;
    let port = config.port().unwrap_or(DEFAULT_PORT);
    print!("{}", hook::script(port, config.api_key.as_deref()));
    Ok(())
}

/// Writes the plist and leaves loading it to the caller: bootstrapping needs
/// the GUI session this may not be running in.
fn install_launchagent(force: bool) -> Result<(), Failure> {
    let program = launchagent::program()?;
    let path = launchagent::path()?;
    let plist = launchagent::plist(&program);

    let existing = std::fs::read_to_string(&path).ok();
    match existing {
        Some(existing) if existing == plist => println!("{} is up to date", path.display()),
        Some(_) if !force => {
            return Err(format!(
                "{} already exists and differs; pass --force to replace it",
                path.display()
            )
            .into());
        }
        _ => {
            let dir = path.parent().ok_or("the plist has nowhere to live")?;
            std::fs::create_dir_all(dir)
                .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
            std::fs::write(&path, &plist)
                .map_err(|err| format!("cannot write {}: {err}", path.display()))?;
            println!("wrote {}", path.display());
        }
    }
    println!("  program: {}", program.display());

    let uid = unsafe { libc::getuid() };
    println!();
    println!("next:");
    println!(
        "  launchctl bootout gui/{uid}/{} 2>/dev/null",
        launchagent::LABEL
    );
    println!("  launchctl bootstrap gui/{uid} {}", path.display());
    Ok(())
}

#[cfg(target_os = "macos")]
fn inhibitor() -> Arc<dyn Inhibitor> {
    Arc::new(caffeine_daemon::caffeinate::Caffeinate::new())
}

#[cfg(not(target_os = "macos"))]
fn inhibitor() -> Arc<dyn Inhibitor> {
    eprintln!("caffeine-daemon: not macOS, holds are tracked but nothing is kept awake");
    Arc::new(caffeine_daemon::caffeinate::Noop::new())
}
