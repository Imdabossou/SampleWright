//! SampleWright from the command line.
//!
//! ```text
//! samplewright-cli models [download [--all]]
//! samplewright-cli separate <song> [--model M] [--cpu]
//! samplewright-cli info <song>
//! samplewright-cli stems <song> [--out DIR] [--model M]
//! samplewright-cli loop <song> --stems bass,drums --bars 9-16 [options] [--out DIR]
//! samplewright-cli slice <song> --stems bass --bars 9-16 [options] [--out DIR]
//! ```
//!
//! Positions: `--bars A-B` (1-based, inclusive) or `--from S --to S` (seconds).
//! Processing: `--hp HZ` `--lp HZ` `--slope 12|24|48` `--gate DB` `--normalize`
//! `--gain DB` `--one-shot` (instead of a loop) `--sensitivity 0..1` (slices).
//! Output: `--format 16|24|32f` `--rate 44100|48000` `--mono` `--separate-stems`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sw_audio::filter::{Pass, Slope};
use sw_audio::wav::Format;
use sw_core::{Kind, Region, Session, separate};
use sw_models::demucs::{self, Device};

struct Args(Vec<String>);

impl Args {
    fn opt(&self, name: &str) -> Option<&str> {
        self.0.iter().position(|a| a == name).and_then(|i| self.0.get(i + 1)).map(String::as_str)
    }

    fn flag(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }

    fn num(&self, name: &str) -> Result<Option<f64>, String> {
        self.opt(name).map(|v| v.parse::<f64>().map_err(|_| format!("{name}: not a number: {v}"))).transpose()
    }

    /// The first argument after the command that is not an option or an option's value.
    fn positional(&self) -> Option<&str> {
        let mut skip = false;
        for a in self.0.iter().skip(1) {
            if skip {
                skip = false;
                continue;
            }
            if a.starts_with("--") {
                skip = !matches!(a.as_str(), "--cpu" | "--all" | "--normalize" | "--one-shot" | "--mono" | "--separate-stems");
                continue;
            }
            return Some(a);
        }
        None
    }
}

fn progress(stage: separate::Stage) {
    eprint!("\r{:<40}", stage.label());
}

fn open(args: &Args) -> Result<Session, String> {
    let song = args.positional().ok_or("which song? (give its path)")?;
    let model = args.opt("--model").unwrap_or("");
    let device = if args.flag("--cpu") { Device::Cpu } else { Device::Auto };
    let missing = separate::missing_models(if model.is_empty() { "htdemucs_6s" } else { model });
    if !missing.is_empty() {
        return Err(format!(
            "missing model files: {} (run `samplewright-cli models download`)",
            missing.iter().map(|a| a.file).collect::<Vec<_>>().join(", ")
        ));
    }
    let t = std::time::Instant::now();
    let s = separate::open(Path::new(song), model, device, &progress).map_err(|e| e.to_string())?;
    eprintln!("\r{:<40}", format!("ready in {:.1} s", t.elapsed().as_secs_f64()));
    Ok(s)
}

fn models(args: &Args) -> Result<(), String> {
    if args.0.get(1).map(String::as_str) == Some("download") {
        for a in sw_models::download::ASSETS.iter().filter(|a| (a.default || args.flag("--all")) && sw_models::find_model(a.file).is_err()) {
            eprintln!("downloading {} ({} MB)", a.file, a.size / 1_000_000);
            sw_models::download::download(a, &mut |done, total| eprint!("\r  {:.0}%", done as f64 * 100.0 / total as f64)).map_err(|e| e.to_string())?;
            eprintln!();
        }
    }
    for a in sw_models::download::ASSETS {
        let at = sw_models::find_model(a.file).map_or("not installed".into(), |p| p.display().to_string());
        println!("{:34} {:>4} MB  {}\n{:34}          {at}", a.file, a.size / 1_000_000, a.what, "");
    }
    match demucs::gpu_name() {
        Ok(n) => println!("GPU: {n}"),
        Err(e) => println!("GPU: {e} (separation runs on the CPU)"),
    }
    Ok(())
}

fn info(s: &Session) {
    println!("{}", s.title);
    println!("  session  {}", s.dir.display());
    println!("  model    {} ({})", s.model, if s.device.is_empty() { "?" } else { &s.device });
    println!("  stems    {}", s.stems.join(", "));
    println!("  length   {:.1} s", s.duration);
    match (s.grid.bpm(), s.grid.beats_per_bar()) {
        (Some(b), Some(n)) => println!("  tempo    {b:.1} bpm, {n} beats per bar, {} bars", s.grid.downbeats.len()),
        _ => println!("  tempo    no steady beat found"),
    }
}

/// The region the arguments describe.
fn region(s: &Session, args: &Args) -> Result<Region, String> {
    let stems: Vec<String> = args.opt("--stems").unwrap_or("mix").split(',').map(|x| x.trim().to_string()).collect();
    let lanes = s.lanes();
    if let Some(bad) = stems.iter().find(|x| !lanes.contains(x)) {
        return Err(format!("no stem '{bad}' (have: {})", lanes.join(", ")));
    }
    let (start, end) = if let Some(bars) = args.opt("--bars") {
        let (a, b) = bars.split_once('-').unwrap_or((bars, bars));
        let (a, b): (usize, usize) = (a.parse().map_err(|_| "--bars A-B")?, b.parse().map_err(|_| "--bars A-B")?);
        let d = &s.grid.downbeats;
        if a == 0 || b < a || a > d.len() {
            return Err(format!("bars run from 1 to {}", d.len()));
        }
        let end = d.get(b).copied().unwrap_or_else(|| s.grid.snap(s.duration, sw_core::grid::Snap::Bar).min(s.duration));
        (d[a - 1], end)
    } else {
        let from = args.num("--from")?.ok_or("give --bars A-B or --from/--to")?;
        let to = args.num("--to")?.ok_or("--to is missing")?;
        (from, to)
    };
    let mut r = Region::new(s.next_region_id(), start, end, stems);
    if args.flag("--one-shot") {
        r.kind = Kind::OneShot;
    }
    let slope = match args.opt("--slope") {
        Some("12") => Slope::Db12,
        Some("48") => Slope::Db48,
        _ => Slope::Db24,
    };
    if let Some(f) = args.num("--hp")? {
        r.chain.filters.highpass = Pass { on: true, freq: f as f32, slope };
    }
    if let Some(f) = args.num("--lp")? {
        r.chain.filters.lowpass = Pass { on: true, freq: f as f32, slope };
    }
    if let Some(t) = args.num("--gate")? {
        r.chain.gate.on = true;
        r.chain.gate.threshold_db = t as f32;
    }
    if let Some(g) = args.num("--gain")? {
        r.chain.gain_db = g as f32;
    }
    r.chain.normalize = args.flag("--normalize");
    if let Some(v) = args.num("--sensitivity")? {
        r.slicing.sensitivity = v as f32;
    }
    Ok(r)
}

fn output(s: &mut Session, args: &Args) -> Result<PathBuf, String> {
    s.export.format = match args.opt("--format") {
        Some("16") => Format::Pcm16,
        Some("32f") => Format::Float32,
        Some("24") | None => Format::Pcm24,
        Some(f) => return Err(format!("--format 16, 24 or 32f, not {f}")),
    };
    if let Some(r) = args.num("--rate")? {
        s.export.sample_rate = r as u32;
    }
    s.export.mono = args.flag("--mono");
    s.export.separate_stems = args.flag("--separate-stems");
    Ok(args.opt("--out").map(PathBuf::from).unwrap_or_else(|| s.export_dir()))
}

fn report(files: &[PathBuf]) {
    for f in files {
        println!("{}", f.display());
    }
    eprintln!("{} file(s) written", files.len());
}

fn run() -> Result<(), String> {
    let args = Args(std::env::args().skip(1).collect());
    match args.0.first().map(String::as_str) {
        Some("models") => models(&args),
        Some("separate" | "info") => {
            info(&open(&args)?);
            Ok(())
        }
        Some("stems") => {
            let mut s = open(&args)?;
            let dir = output(&mut s, &args)?;
            let stems = s.load_audio().map_err(|e| e.to_string())?;
            report(&s.export_stems(&stems, &s.stems, &dir).map_err(|e| e.to_string())?);
            Ok(())
        }
        Some(cmd @ ("loop" | "slice")) => {
            let mut s = open(&args)?;
            let dir = output(&mut s, &args)?;
            let mut r = region(&s, &args)?;
            if cmd == "slice" {
                r.kind = Kind::Slices;
            }
            eprintln!("{} {:.2}-{:.2} s ({})", r.kind.label(), r.start, r.end, s.position_label(&r));
            let stems = s.load_audio().map_err(|e| e.to_string())?;
            report(&s.export_region(&stems, &r, &dir).map_err(|e| e.to_string())?);
            Ok(())
        }
        _ => {
            eprintln!("{}", include_str!("main.rs").lines().take_while(|l| l.starts_with("//!")).map(|l| l.trim_start_matches("//!").trim_start_matches(' ')).collect::<Vec<_>>().join("\n").trim().replace("```text\n", "").replace("\n```", ""));
            Err(String::new())
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}");
            }
            ExitCode::FAILURE
        }
    }
}
