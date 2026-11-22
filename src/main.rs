#![cfg_attr(all(not(debug_assertions), not(test)), windows_subsystem = "windows")]

mod app;
mod capture;
mod codec;
mod crypto;
mod input;
mod proto;
mod selftest;
mod session;
mod util;

use app::StartMode;
use std::io::Write as _;
use std::process::exit;

const USAGE: &str = "\
DeskBeam - portable peer-to-peer remote desktop

USAGE:
  deskbeam                         Open the GUI (Host / Connect tabs)
  deskbeam --host [--port N]       Start hosting immediately
  deskbeam --viewer ADDR [--code CODE]
                                   Connect immediately (ADDR may be host:port)
  deskbeam --selftest              Run the loopback end-to-end test
  deskbeam --version               Print the version

The default port is 7878. The host must click Accept for every session.
";

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version" || a == "-V") {
        let _ = writeln!(std::io::stdout(), "deskbeam {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        let _ = writeln!(std::io::stdout(), "{USAGE}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--selftest") {
        let result = selftest::run();
        let text = match &result {
            Ok(()) => "PASS".to_string(),
            Err(e) => format!("FAIL: {e:#}"),
        };
        let _ = std::fs::write("deskbeam-selftest.txt", format!("{text}\n"));
        let _ = writeln!(std::io::stdout(), "DeskBeam self-test: {text}");
        if result.is_err() {
            exit(1);
        }
        return Ok(());
    }

    capture::init_dpi_awareness();

    let mode = match parse_args(&args) {
        Ok(mode) => mode,
        Err(message) => {
            let _ = writeln!(std::io::stderr(), "{message}\n\n{USAGE}");
            exit(2);
        }
    };

    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("DeskBeam")
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([800.0, 540.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "DeskBeam",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(app::DeskBeamApp::new(cc, mode)) as Box<dyn eframe::App>)
        }),
    )
}

fn parse_args(args: &[String]) -> Result<StartMode, String> {
    let value_after = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let host = args.iter().any(|a| a == "--host");
    let viewer_flag = args.iter().any(|a| a == "--viewer");
    let port_flag = args.iter().any(|a| a == "--port");
    let code_flag = args.iter().any(|a| a == "--code");
    let viewer = value_after("--viewer");
    if viewer_flag && viewer.is_none() {
        return Err("--viewer requires an address".into());
    }
    if port_flag && value_after("--port").is_none() {
        return Err("--port requires a number".into());
    }
    if code_flag && value_after("--code").is_none() {
        return Err("--code requires a value".into());
    }
    let port = match value_after("--port") {
        Some(p) => Some(
            p.parse::<u16>()
                .map_err(|_| format!("invalid port: {p}"))?,
        ),
        None => None,
    };
    let code = value_after("--code");

    match (host, viewer) {
        (true, Some(_)) => Err("use either --host or --viewer, not both".into()),
        (true, None) => Ok(StartMode::Host {
            port: port.unwrap_or(session::DEFAULT_PORT),
        }),
        (false, Some(address)) => Ok(StartMode::Viewer { address, code }),
        (false, None) => {
            if code.is_some() {
                return Err("--code requires --viewer".into());
            }
            Ok(StartMode::Gui)
        }
    }
}
