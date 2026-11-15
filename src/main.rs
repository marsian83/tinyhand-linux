//! TinyHand for Linux: a tiny hand on a stick that points at things on your screen.
//! Unofficial port of TinyHand for macOS by Kamran Ahmed (MIT).
//!
//! No daemon: `tinyhand toggle` starts the pointer, and running it again (or `off`)
//! ends the process entirely, so nothing is left in memory while it's off.

use std::{path::PathBuf, process::ExitCode};

use tinyhand::{backend, bench, config, config::Config, handmaker, ipc, setup, theme};

const USAGE: &str = "\
usage: tinyhand [COMMAND] [OPTIONS]

commands:
  on              turn the pointer on (default)
  off             turn it off
  toggle          on if off, off if on (bind this to a key)
  clear           clear the drawing
  real [on|off]   click for real: let clicks through to apps (bind press/release)
  hand NAME       switch hand (e.g. coral, robot, laser, or a PNG path)
  sound [on|off]  tap sound
  hands           list built-in and your own hands
  prompt [IDEA]   print a prompt for ChatGPT or another image tool to draw a hand
                  (IDEA is the style, e.g. knitted wool in mustard yellow)
  make-hand IMAGE [NAME]
                  check the PNG it gave you, put it on a stick and add it
                  (prints the new hand's id)
  remove-hand ID  delete one of your own hands
  setup           print keybinding snippets for your desktop
  settings        open the settings window (tinyhand-settings)
  bench           offscreen rendering benchmark

options (override ~/.config/tinyhand/config):
  --hand NAME  --size 0.08-0.20  --no-shadow  --no-sound  --tap-sound NAME
  --fade SECONDS (0 keeps drawings)  --ink COLOUR|match  --stats";

struct Args {
    cmd: String,
    arg: Option<String>,
    cfg: Config,
    out: Option<PathBuf>,
    /// Everything after `prompt`, `make-hand` or `remove-hand`.
    rest: Vec<String>,
}

/// Why parsing stopped short of running a command.
enum Stop {
    /// --help or --version: print this and exit successfully.
    Info(String),
    /// A mistake on the command line.
    Usage(String),
}

fn parse() -> Result<Args, Stop> {
    let mut a = Args { cmd: "on".into(), arg: None, cfg: Config::load(), out: None, rest: Vec::new() };
    let mut it = std::env::args().skip(1).peekable();
    let set = |cfg: &mut Config, k: &str, v: Option<String>| -> Result<(), Stop> {
        let v = v.ok_or_else(|| Stop::Usage(format!("--{} needs a value", k.replace('_', "-"))))?;
        cfg.set(k, &v).map_err(Stop::Usage)
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "on" | "off" | "toggle" | "clear" | "bench" | "hands" | "setup" | "settings" => a.cmd = arg,
            "real" | "sound" | "hand" => {
                a.cmd = arg;
                a.arg = it.next_if(|v| !v.starts_with("--"));
            }
            "prompt" | "make-hand" | "remove-hand" => {
                a.cmd = arg;
                a.rest = it.by_ref().collect();
            }
            "--hand" => set(&mut a.cfg, "hand", it.next())?,
            "--size" => set(&mut a.cfg, "size", it.next())?,
            "--tap-sound" => set(&mut a.cfg, "tap_sound", it.next())?,
            "--fade" => set(&mut a.cfg, "fade_seconds", it.next())?,
            "--ink" => set(&mut a.cfg, "ink", it.next())?,
            "--no-shadow" => a.cfg.shadow = false,
            "--no-sound" => a.cfg.sound = false,
            "--stats" => a.cfg.stats = true,
            "--out" => a.out = Some(it.next().ok_or_else(|| Stop::Usage("--out needs a path".into()))?.into()),
            "-h" | "--help" => return Err(Stop::Info(USAGE.into())),
            "-V" | "--version" => return Err(Stop::Info(format!("tinyhand {}", env!("CARGO_PKG_VERSION")))),
            other => return Err(Stop::Usage(format!("unknown argument {other}\n\n{USAGE}"))),
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(a) => a,
        Err(Stop::Info(text)) => {
            println!("{text}");
            return ExitCode::SUCCESS;
        }
        Err(Stop::Usage(e)) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let result = match args.cmd.as_str() {
        "bench" => bench::run(&args.cfg, args.out.as_deref()),
        "setup" => {
            print!("{}", setup::snippet());
            Ok(())
        }
        "settings" => settings(),
        "hands" => {
            for (id, title) in config::all_hands() {
                println!("{id:15} {title}");
            }
            Ok(())
        }
        "prompt" => {
            println!("{}", handmaker::prompt(&args.rest.join(" ")));
            Ok(())
        }
        "make-hand" => make_hand(&args.rest),
        "remove-hand" => match args.rest.as_slice() {
            [id] if handmaker::get(id).is_some() => handmaker::remove(id),
            [id] => Err(format!("no custom hand '{id}' (see `tinyhand hands`)")),
            _ => Err("usage: tinyhand remove-hand ID".into()),
        },
        "off" | "clear" => {
            // Nothing running means nothing to turn off or clear.
            let _ = ipc::send(&args.cmd);
            Ok(())
        }
        "real" | "sound" | "hand" => {
            let line = match &args.arg {
                // The pointer runs elsewhere (its working directory is the hotkey
                // daemon's): send image paths as absolute paths.
                Some(v) if args.cmd == "hand" => format!("hand {}", absolute_if_file(v)),
                Some(v) => format!("{} {v}", args.cmd),
                None => args.cmd.clone(),
            };
            ipc::send(&line).map_err(|_| "tinyhand isn't running".to_string())
        }
        "toggle" if ipc::running() => ipc::send("off").map_err(|e| e.to_string()),
        _ => start(args.cfg),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tinyhand: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `tinyhand make-hand IMAGE [NAME...]`: check the image, put it on a stick, save it and
/// print the new id.
fn make_hand(rest: &[String]) -> Result<(), String> {
    let (image, name) = rest.split_first().ok_or("usage: tinyhand make-hand IMAGE [NAME]")?;
    let path = std::path::Path::new(image);
    let made = handmaker::make(path)?;
    let name = if name.is_empty() { handmaker::suggested_name(path) } else { name.join(" ") };
    let hand = handmaker::add(&made, &name)?;
    println!("{}", hand.id);
    eprintln!(
        "Added \"{}\" (ink #{:06x}). Use it with `tinyhand hand {}` or `hand = {}` in the config.",
        hand.name, hand.ink, hand.id, hand.id
    );
    Ok(())
}

/// Open the settings window: `tinyhand-settings` next to this binary, else from PATH.
fn settings() -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    const NAME: &str = "tinyhand-settings";
    let beside = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join(NAME))).filter(|p| p.is_file());
    let mut cmd = std::process::Command::new(beside.unwrap_or_else(|| PathBuf::from(NAME)));
    // exec only returns on failure.
    let err = cmd.exec();
    Err(match err.kind() {
        std::io::ErrorKind::NotFound => {
            format!("{NAME} isn't installed (it comes with the TinyHand package; from source: cargo build --release -p {NAME})")
        }
        _ => format!("can't start {NAME}: {err}"),
    })
}

fn start(cfg: Config) -> Result<(), String> {
    let server = match ipc::Server::bind() {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return Ok(()),
        Err(e) => return Err(format!("control socket: {e}")),
    };
    theme::start();
    backend::run(cfg, &server)
}

/// `v` as an absolute path if it names an existing file, else unchanged (a hand id).
fn absolute_if_file(v: &str) -> String {
    match std::fs::canonicalize(v) {
        Ok(p) if p.is_file() => p.to_string_lossy().into_owned(),
        _ => v.to_string(),
    }
}
