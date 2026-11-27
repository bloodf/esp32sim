//! esp32sim — ESP32 board emulator with circuit simulation, console, GUI and IDE.

use clap::{Parser, Subcommand};
use esp32sim::{cli, console, project, server, testrun};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "esp32sim", version, about = "ESP32 emulator: run real firmware (.bin/.elf) against a simulated circuit")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a project (or a firmware image) with an interactive serial console
    Run {
        /// project file (esp32sim.yaml), project directory or firmware (.bin/.elf)
        target: PathBuf,
        /// real-time factor (1 = real time, 0 = as fast as possible)
        #[arg(long)]
        speed: Option<f64>,
        /// stop after this many emulated seconds
        #[arg(long)]
        seconds: Option<f64>,
        /// line ending sent after each console line: auto, lf, crlf, cr, none
        /// (auto = LF, or CR once MicroPython's banner is seen)
        #[arg(long, default_value = "auto")]
        eol: String,
    },
    /// Run the project's `test:` scenario headless (exit code 0 = pass)
    Test {
        target: PathBuf,
        /// overall timeout in emulated seconds
        #[arg(long, default_value_t = 60.0)]
        timeout: f64,
        /// print serial output while running
        #[arg(long)]
        verbose: bool,
    },
    /// Start the web GUI (circuit editor, simulator view and IDE)
    Gui {
        /// project file or directory to open
        target: Option<PathBuf>,
        #[arg(long, default_value_t = 8732)]
        port: u16,
        /// do not open a browser window
        #[arg(long)]
        no_browser: bool,
        /// address to listen on (use 0.0.0.0 to allow other machines)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
    /// Create a new PlatformIO + esp32sim project
    New {
        dir: PathBuf,
        /// template: blink, button, oled, lcd, keypad, neopixel, sensors, nfc, rfid, sdlogger, gps
        #[arg(long, default_value = "blink")]
        template: String,
    },
    /// List supported parts, or show the pins and properties of one part type
    Parts {
        /// part type (e.g. `ssd1306`) to describe in detail
        part: Option<String>,
    },
    /// Show information about a firmware image
    Info { firmware: PathBuf },
}

fn main() {
    let cli = Cli::parse();
    let r = match cli.cmd {
        Command::Run { target, speed, seconds, eol } => console::run(&target, speed, seconds, &eol),
        Command::Test { target, timeout, verbose } => testrun::run(&target, timeout, verbose),
        Command::Gui { target, port, no_browser, host } => {
            server::run(server::ServerOptions { target, port, open_browser: !no_browser, host, ..Default::default() })
        }
        Command::New { dir, template } => project::create(&dir, &template).map(|p| {
            println!("created {}", p.display());
            println!("next: cd {} && pio run && esp32sim run .", dir.display());
            0
        }),
        Command::Parts { part: None } => cli::list_parts(&mut std::io::stdout().lock()),
        Command::Parts { part: Some(p) } => cli::describe_part(&p, &mut std::io::stdout().lock()),
        Command::Info { firmware } => cli::info(&firmware),
    };
    match r {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(2);
        }
    }
}
