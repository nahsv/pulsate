//! Pulsate's command-line entry point: parse the CLI, build the runtime, and
//! dispatch to the command implementations in `pulsate-cli` (`docs/13-cli.md`).
#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Pulsate — one binary, one config, one command.
#[derive(Debug, Parser)]
#[command(version, about = "The Pulsate application gateway")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the gateway from a config file.
    Up {
        /// Path to the config file.
        #[arg(default_value = "pulsate.flow")]
        config: PathBuf,
        /// Plain-HTTP listen address.
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
        /// TLS listen address (requires --cert and --key).
        #[arg(long)]
        tls_listen: Option<SocketAddr>,
        /// PEM certificate chain for the TLS listener.
        #[arg(long)]
        cert: Option<PathBuf>,
        /// PEM private key for the TLS listener.
        #[arg(long)]
        key: Option<PathBuf>,
        /// Prometheus metrics listen address (set to "off" to disable).
        #[arg(long, default_value = "127.0.0.1:9100")]
        metrics: String,
        /// Admin API + dashboard listen address (set to "off" to disable).
        #[arg(long, default_value = "127.0.0.1:9180")]
        admin: String,
        /// Admin bearer token (generated if omitted).
        #[arg(long)]
        admin_token: Option<String>,
        /// Advertise HTTP/3 via Alt-Svc on this UDP port.
        #[arg(long)]
        http3_port: Option<u16>,
    },
    /// Load and run a WASM plugin.
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Import an nginx/Caddy/HAProxy/Apache config into Flow.
    Import {
        /// Source format: nginx, caddy, haproxy, or apache.
        format: String,
        /// Path to the foreign config file.
        file: PathBuf,
    },
    /// Validate a config file without starting.
    Validate {
        /// Path to the config file.
        #[arg(default_value = "pulsate.flow")]
        config: PathBuf,
    },
    /// Inspect configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Print build and runtime information.
    Info,
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Validate, then print the typed configuration model.
    Dump {
        /// Path to the config file.
        #[arg(default_value = "pulsate.flow")]
        config: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum PluginAction {
    /// Load a `.wasm`/`.wat` plugin and call its `eval(input)`.
    Run {
        /// Path to the plugin module.
        file: PathBuf,
        /// Integer input passed to `eval`.
        #[arg(default_value_t = 1)]
        input: i32,
    },
}

/// Parse the command line and run the chosen command.
#[must_use]
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Info) {
        Command::Info => {
            print_info();
            ExitCode::SUCCESS
        }
        Command::Validate { config } => emit(&pulsate_cli::validate(&config)),
        Command::Config {
            action: ConfigAction::Dump { config },
        } => emit(&pulsate_cli::config_dump(&config)),
        Command::Up {
            config,
            listen,
            tls_listen,
            cert,
            key,
            metrics,
            admin,
            admin_token,
            http3_port,
        } => run_up(RunUp {
            config,
            listen,
            tls_listen,
            cert,
            key,
            metrics,
            admin,
            admin_token,
            http3_port,
        }),
        Command::Plugin {
            action: PluginAction::Run { file, input },
        } => emit(&pulsate_cli::plugin_run(&file, input)),
        Command::Import { format, file } => emit(&pulsate_cli::import_config(&format, &file)),
    }
}

/// Parsed arguments for `run_up`.
struct RunUp {
    config: PathBuf,
    listen: SocketAddr,
    tls_listen: Option<SocketAddr>,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    metrics: String,
    admin: String,
    admin_token: Option<String>,
    http3_port: Option<u16>,
}

/// Build the runtime and serve. Validates that TLS flags come as a complete set.
fn run_up(args: RunUp) -> ExitCode {
    let RunUp {
        config,
        listen,
        tls_listen,
        cert,
        key,
        metrics,
        admin,
        admin_token,
        http3_port,
    } = args;
    let tls = match (tls_listen, cert, key) {
        (None, None, None) => None,
        (Some(listen), Some(cert), Some(key)) => {
            Some(pulsate_cli::TlsOptions { listen, cert, key })
        }
        _ => {
            eprintln!("pulsate: --tls-listen, --cert, and --key must be provided together");
            return ExitCode::from(64); // usage error (sysexits)
        }
    };

    let metrics = match parse_optional_addr(&metrics, "--metrics") {
        Ok(a) => a,
        Err(code) => return code,
    };
    let admin = match parse_optional_addr(&admin, "--admin") {
        Ok(a) => a,
        Err(code) => return code,
    };

    let rt = match pulsate_rt::Runtime::new(None) {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("pulsate: failed to start runtime: {e}");
            return ExitCode::from(1);
        }
    };
    let code = rt.block_on(pulsate_cli::up(pulsate_cli::UpOptions {
        config,
        listen,
        tls,
        metrics,
        admin,
        admin_token,
        http3_port,
    }));
    ExitCode::from(code)
}

/// Parse an "addr or `off`" flag into an optional socket address.
fn parse_optional_addr(value: &str, flag: &str) -> Result<Option<SocketAddr>, ExitCode> {
    if value.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    match value.parse::<SocketAddr>() {
        Ok(addr) => Ok(Some(addr)),
        Err(e) => {
            eprintln!("pulsate: invalid {flag} address {value:?}: {e}");
            Err(ExitCode::from(64))
        }
    }
}

/// Print a command [`pulsate_cli::Outcome`] and translate its code to an [`ExitCode`].
fn emit(outcome: &pulsate_cli::Outcome) -> ExitCode {
    if !outcome.stdout.is_empty() {
        print!("{}", outcome.stdout);
    }
    if !outcome.stderr.is_empty() {
        eprint!("{}", outcome.stderr);
    }
    ExitCode::from(outcome.code)
}

/// Print version and a short list of common commands.
fn print_info() {
    let bin = "pulsate";
    let version = env!("CARGO_PKG_VERSION");
    println!("{bin} {version} — one binary, one config, one command");
    println!();
    println!("  {bin} up <config>          serve a gateway");
    println!("  {bin} validate <config>    check a config without starting");
    println!("  {bin} import <fmt> <file>  translate nginx/caddy/haproxy/apache to Flow");
    println!("  {bin} plugin run <file>    run a WASM plugin");
}
