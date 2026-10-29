//! The SampleWright window: menus and toolbar, the timeline, and a side panel
//! to edit the selected region and export.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText, Ui};
use sw_audio::filter::{BandKind, Slope};
use sw_audio::peaks::Peaks;
use sw_audio::wav::Format;
use sw_core::grid::Snap;
use sw_core::separate::{self, Stage};
use sw_core::slice::{self, Slice};
use sw_core::{Kind, Region, Session, Stems};
use sw_models::demucs::Device;
use sw_models::download::Asset;

use crate::filter_view::{self, Spectrum};
use crate::player::Player;
use crate::theme;
use crate::timeline::{LaneState, Timeline, capitalise};

/// The separation models offered, with what they are good for.
const MODELS: [(&str, &str, &str); 3] = [
    ("htdemucs_6s", "6 stems", "drums, bass, vocals, guitar, piano and other. Best for guitar parts. ~55 MB"),
    ("htdemucs_ft", "4 stems, best quality", "drums, bass, vocals and other, cleanest separation, slowest. ~336 MB"),
    ("htdemucs", "4 stems, fast", "drums, bass, vocals and other. ~84 MB"),
];

const SHORTCUTS: &str = "Space          play / pause (stops a preview)
Drag on lanes  select (snaps to bars or beats; hold Alt for none)
Enter          make a region from the selection
P              preview the selected region on loop
L              loop playback over the selection / region
Delete         delete the selected region
Esc            clear the selection, stop the preview
Ctrl+E         export the selected region
Ctrl+O         open a song
Ctrl+scroll    zoom (or pinch), scroll to move along the song
Double-click   preview a region";

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Prefs {
    recent: Vec<PathBuf>,
    model: String,
    theme: String,
    cpu_only: bool,
}

enum JobResult {
    Opened(Box<Session>),
    Loaded(Stems, Vec<Peaks>),
    Exported(Vec<PathBuf>, PathBuf),
    Downloaded(Option<(PathBuf, String)>),
    Failed(String),
}

struct Job {
    label: String,
    progress: Arc<Mutex<(f32, String)>>,
    rx: Receiver<JobResult>,
}

enum Dialog {
    Message(String, String),
    Separate { song: PathBuf, model: String },
    Models { assets: Vec<&'static Asset>, then: Option<(PathBuf, String)> },
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Tab {
    Region,
    Song,
}

pub struct App {
    session: Option<Session>,
    stems: Option<Stems>,
    lanes: Vec<LaneState>,
    player: Player,
    timeline: Timeline,
    selected: Option<u64>,
    /// (region id, settings hash, cuts)
    slices: Option<(u64, u64, Vec<Slice>)>,
    /// (region hash, spectrum)
    spectrum: Option<(u64, Spectrum)>,
    job: Option<Job>,
    dialog: Option<Dialog>,
    prefs: Prefs,
    status: String,
    dirty: Option<Instant>,
    /// Re-render the preview after edits settle.
    preview_due: Option<Instant>,
    /// Region (or slice) being previewed.
    previewing: Option<(u64, Option<usize>)>,
    looping: bool,
    tab: Tab,
    last_export: Option<PathBuf>,
    gpu: Arc<Mutex<Option<String>>>,
    title: String,
}

fn hash_of<T: std::fmt::Debug>(v: &T) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{v:?}").hash(&mut h);
    h.finish()
}

fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let cmd = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

fn fmt_time(t: f64) -> String {
    format!("{}:{:05.2}", (t.max(0.0) / 60.0) as u64, t.max(0.0) % 60.0)
}

const AUDIO_EXTS: [&str; 9] = ["mp3", "flac", "wav", "ogg", "m4a", "aac", "aif", "aiff", "opus"];

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> App {
        theme::install(&cc.egui_ctx);
        let prefs: Prefs = cc.storage.and_then(|s| eframe::get_value(s, "prefs")).unwrap_or_default();
        cc.egui_ctx.set_theme(match prefs.theme.as_str() {
            "light" => egui::ThemePreference::Light,
            "dark" => egui::ThemePreference::Dark,
            _ => egui::ThemePreference::System,
        });
        let gpu = Arc::new(Mutex::new(None));
        {
            let gpu = gpu.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let name = match sw_models::demucs::gpu_name() {
                    Ok(n) => format!("GPU: {n}"),
                    Err(_) => "No usable GPU: separation runs on the CPU (slower)".into(),
                };
                *gpu.lock().unwrap_or_else(|e| e.into_inner()) = Some(name);
                ctx.request_repaint();
            });
        }
        let player = Player::new();
        let mut app = App {
            session: None,
            stems: None,
            lanes: Vec::new(),
            status: player.error.clone().map(|e| format!("No audio output: {e}")).unwrap_or_default(),
            player,
            timeline: Timeline::default(),
            selected: None,
            slices: None,
            spectrum: None,
            job: None,
            dialog: None,
            prefs,
            dirty: None,
            preview_due: None,
            previewing: None,
            looping: false,
            tab: Tab::Region,
            last_export: None,
            gpu,
            title: String::new(),
        };
        if app.prefs.model.is_empty() {
            app.prefs.model = MODELS[0].0.into();
        }
        let missing: Vec<_> = separate::missing_models(&app.prefs.model);
        if let Some(p) = open {
            app.open_song(&cc.egui_ctx, &p);
        } else if !missing.is_empty() {
            app.dialog = Some(Dialog::Models { assets: missing, then: None });
        }
        app
    }

    fn message(&mut self, title: &str, text: impl Into<String>) {
        self.dialog = Some(Dialog::Message(title.into(), text.into()));
    }

    fn changed(&mut self) {
        self.dirty = Some(Instant::now());
        if self.player.preview_on {
            self.preview_due = Some(Instant::now() + Duration::from_millis(120));
        }
    }

    fn save_now(&mut self) {
        if let Some(s) = &self.session
            && self.dirty.take().is_some()
            && let Err(e) = s.save()
        {
            self.status = format!("Could not save the session: {e}");
        }
    }

    fn spawn<F>(&mut self, ctx: &egui::Context, label: impl Into<String>, work: F)
    where
        F: FnOnce(&(dyn Fn(f32, String) + Sync)) -> JobResult + Send + 'static,
    {
        let progress = Arc::new(Mutex::new((0.0f32, String::new())));
        let (tx, rx) = channel();
        let p = progress.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let c2 = c.clone();
            let report = move |f: f32, s: String| {
                *p.lock().unwrap_or_else(|e| e.into_inner()) = (f, s);
                c2.request_repaint();
            };
            let r = work(&report);
            let _ = tx.send(r);
            c.request_repaint();
        });
        self.job = Some(Job { label: label.into(), progress, rx });
    }

    // ------------------------------------------------------------ opening

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(p) = rfd::FileDialog::new().add_filter("Audio", &AUDIO_EXTS).set_title("Open a song").pick_file() {
            self.open_song(ctx, &p);
        }
    }

    /// Opens a song: straight away if it was separated before, else asks which model to use.
    fn open_song(&mut self, ctx: &egui::Context, song: &Path) {
        if self.job.is_some() {
            return;
        }
        let known = sw_core::session_dir_for(song).ok().and_then(|d| Session::load(&d).ok());
        match known {
            Some(s) if s.stems.iter().all(|st| s.stem_path(st).is_file()) => {
                let model = s.model.clone();
                self.run_separation(ctx, song.to_path_buf(), model);
            }
            _ if song.is_file() => self.dialog = Some(Dialog::Separate { song: song.to_path_buf(), model: self.prefs.model.clone() }),
            _ => self.message("Open song", format!("{} can't be read.", song.display())),
        }
    }

    fn run_separation(&mut self, ctx: &egui::Context, song: PathBuf, model: String) {
        let missing = separate::missing_models(&model);
        if !missing.is_empty() {
            self.dialog = Some(Dialog::Models { assets: missing, then: Some((song, model)) });
            return;
        }
        self.save_now();
        let device = if self.prefs.cpu_only { Device::Cpu } else { Device::Auto };
        let name = song.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.spawn(ctx, format!("Opening {name}"), move |report| {
            let r = separate::open(&song, &model, device, &|st: Stage| report(st.fraction() as f32, st.label()));
            match r {
                Ok(s) => JobResult::Opened(Box::new(s)),
                Err(e) => JobResult::Failed(format!("Could not open {}:\n{e}", song.display())),
            }
        });
    }

    fn opened(&mut self, ctx: &egui::Context, s: Session) {
        self.player.stop();
        self.previewing = None;
        self.prefs.recent.retain(|p| p != &s.source);
        self.prefs.recent.insert(0, s.source.clone());
        self.prefs.recent.truncate(10);
        let keep_selection = self.session.as_ref().is_some_and(|old| old.dir == s.dir);
        if !keep_selection {
            self.selected = s.regions.first().map(|r| r.id);
            self.timeline.reset(s.duration);
        }
        self.slices = None;
        self.spectrum = None;
        self.stems = None;
        self.timeline.peaks.clear();
        let s2 = s.clone();
        self.session = Some(s);
        self.spawn(ctx, "Loading stems", move |report| {
            report(0.2, "Loading stems".into());
            match s2.load_audio() {
                Ok(stems) => {
                    use rayon::prelude::*;
                    let peaks = s2.lanes().par_iter().map(|l| stems.get(l).map(|a| Peaks::new(&a.mono())).unwrap_or_default()).collect();
                    JobResult::Loaded(stems, peaks)
                }
                Err(e) => JobResult::Failed(format!("Could not load the stems: {e}")),
            }
        });
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let Ok(result) = job.rx.try_recv() else { return };
        self.job = None;
        match result {
            JobResult::Opened(s) => {
                let s = *s;
                self.status = format!("{}: {} stems with {}", s.title, s.stems.len(), s.model);
                self.opened(ctx, s);
            }
            JobResult::Loaded(stems, peaks) => {
                if let Some(s) = &self.session {
                    let lanes: Vec<_> = s.lanes().iter().filter_map(|l| stems.get(l).cloned()).collect();
                    self.lanes = vec![LaneState::default(); lanes.len()];
                    self.player.load(lanes);
                }
                self.timeline.peaks = peaks;
                self.stems = Some(stems);
            }
            JobResult::Exported(files, dir) => {
                self.status = format!("Exported {} file{} to {}", files.len(), if files.len() == 1 { "" } else { "s" }, dir.display());
                self.last_export = Some(dir);
            }
            JobResult::Downloaded(then) => {
                self.status = "Models downloaded".into();
                if let Some((song, model)) = then {
                    self.run_separation(ctx, song, model);
                }
            }
            JobResult::Failed(e) => self.message("Something went wrong", e),
        }
    }

    fn download(&mut self, ctx: &egui::Context, assets: Vec<&'static Asset>, then: Option<(PathBuf, String)>) {
        self.spawn(ctx, "Downloading models", move |report| {
            let total: u64 = assets.iter().map(|a| a.size).sum();
            let mut before = 0u64;
            for a in &assets {
                let r = sw_models::download::download(a, &mut |done, _| {
                    report((before + done) as f32 / total.max(1) as f32, format!("Downloading {} ({:.0} MB)", a.what, a.size as f64 / 1e6));
                });
                if let Err(e) = r {
                    return JobResult::Failed(e.to_string());
                }
                before += a.size;
            }
            JobResult::Downloaded(then)
        });
    }

    // ------------------------------------------------------------ regions

    fn region(&self) -> Option<&Region> {
        let id = self.selected?;
        self.session.as_ref()?.regions.iter().find(|r| r.id == id)
    }

    fn add_region(&mut self) {
        let (Some(s), Some(sel)) = (&mut self.session, self.timeline.selection) else { return };
        if sel.end - sel.start < 0.01 {
            return;
        }
        let names = s.lanes();
        let stems: Vec<String> = names[sel.lanes.0..=sel.lanes.1.min(names.len() - 1)].to_vec();
        let mut r = Region::new(s.next_region_id(), sel.start, sel.end, stems);
        if r.duration() < 1.0 {
            r.kind = Kind::OneShot;
        }
        self.selected = Some(r.id);
        s.regions.push(r);
        self.timeline.selection = None;
        self.tab = Tab::Region;
        self.changed();
    }

    fn delete_region(&mut self) {
        let (Some(s), Some(id)) = (&mut self.session, self.selected) else { return };
        if self.previewing.is_some_and(|p| p.0 == id) {
            self.player.stop_preview();
            self.previewing = None;
        }
        s.regions.retain(|r| r.id != id);
        self.selected = None;
        self.changed();
    }

    fn preview(&mut self, id: u64, slice_index: Option<usize>) {
        let (Some(s), Some(stems)) = (&self.session, &self.stems) else { return };
        let Some(r) = s.regions.iter().find(|r| r.id == id) else { return };
        let (audio, start) = match slice_index {
            Some(i) => {
                let Some(cut) = self.slices.as_ref().filter(|c| c.0 == id).and_then(|c| c.2.get(i)) else { return };
                let p = r.process(stems, &r.stems);
                (slice::render(r, &p, cut), cut.start)
            }
            None => (r.render(stems, &r.stems), r.start),
        };
        self.player.preview(audio, start);
        self.previewing = Some((id, slice_index));
    }

    fn stop_preview(&mut self) {
        self.player.stop_preview();
        self.previewing = None;
        self.preview_due = None;
    }

    fn export(&mut self, ctx: &egui::Context, ids: Vec<u64>) {
        let (Some(s), Some(stems)) = (&self.session, &self.stems) else { return };
        if self.job.is_some() {
            return;
        }
        let (s, stems) = (s.clone(), stems.clone());
        let dir = s.export_dir();
        self.spawn(ctx, "Exporting", move |report| {
            let regions: Vec<&Region> = s.regions.iter().filter(|r| ids.contains(&r.id)).collect();
            let mut files = Vec::new();
            for (k, r) in regions.iter().enumerate() {
                report(k as f32 / regions.len().max(1) as f32, format!("Exporting {}", r.name));
                match s.export_region(&stems, r, &dir) {
                    Ok(f) => files.extend(f),
                    Err(e) => return JobResult::Failed(format!("Export failed: {e}")),
                }
            }
            JobResult::Exported(files, dir)
        });
    }

    fn export_stems(&mut self, ctx: &egui::Context) {
        let (Some(s), Some(stems)) = (&self.session, &self.stems) else { return };
        if self.job.is_some() {
            return;
        }
        let (s, stems) = (s.clone(), stems.clone());
        let dir = s.export_dir();
        self.spawn(ctx, "Exporting stems", move |report| {
            report(0.1, "Writing stems".into());
            match s.export_stems(&stems, &s.stems, &dir) {
                Ok(f) => JobResult::Exported(f, dir),
                Err(e) => JobResult::Failed(format!("Export failed: {e}")),
            }
        });
    }

    fn update_gains(&mut self) {
        let any_solo = self.lanes.iter().any(|l| l.solo);
        let g = self.lanes.iter().map(|l| if l.mute || (any_solo && !l.solo) { 0.0 } else { l.gain }).collect();
        self.player.set_gains(g);
    }

    fn update_loop(&mut self) {
        let range = if !self.looping {
            None
        } else if let Some(sel) = self.timeline.selection {
            Some((sel.start, sel.end))
        } else {
            self.region().map(|r| (r.start, r.end))
        };
        self.player.set_loop(range);
    }

    fn toggle_play(&mut self) {
        if self.player.preview_on {
            self.stop_preview();
        } else if self.player.is_playing() {
            self.player.pause();
        } else {
            self.player.play();
        }
    }

    // ------------------------------------------------------------ input

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::{Key, Modifiers};
        let pressed = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k));
        let cmd = |k: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, k));
        if cmd(Key::O) {
            self.open_dialog(ctx);
        }
        if cmd(Key::E)
            && let Some(id) = self.selected
        {
            self.export(ctx, vec![id]);
        }
        if self.session.is_none() {
            return;
        }
        if pressed(Key::Space) {
            self.toggle_play();
        }
        if pressed(Key::Enter) {
            self.add_region();
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            self.delete_region();
        }
        if pressed(Key::Escape) {
            self.timeline.selection = None;
            self.stop_preview();
        }
        if pressed(Key::P)
            && let Some(id) = self.selected
        {
            self.preview(id, None);
        }
        if pressed(Key::L) {
            self.looping = !self.looping;
        }
        let dur = self.session.as_ref().map_or(0.0, |s| s.duration);
        if pressed(Key::Plus) || pressed(Key::Equals) {
            self.timeline.zoom(1.25, None, dur);
        }
        if pressed(Key::Minus) {
            self.timeline.zoom(0.8, None, dur);
        }
        if pressed(Key::Home) {
            self.player.seek(0.0);
            self.timeline.scroll_t = 0.0;
        }
    }

    // ------------------------------------------------------------ panels

    fn menus(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open Song…").clicked() {
                    self.open_dialog(&ctx);
                }
                ui.menu_button("Open Recent", |ui| {
                    let recent: Vec<PathBuf> = self.prefs.recent.iter().filter(|p| p.is_file()).cloned().collect();
                    if recent.is_empty() {
                        ui.label("No recent songs");
                    }
                    for p in recent {
                        if ui.button(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).on_hover_text(p.display().to_string()).clicked() {
                            self.open_song(&ctx, &p);
                        }
                    }
                });
                ui.separator();
                let has = self.stems.is_some();
                if ui.add_enabled(has && self.selected.is_some(), egui::Button::new("Export Selected Region")).clicked()
                    && let Some(id) = self.selected
                {
                    self.export(&ctx, vec![id]);
                }
                let all: Vec<u64> = self.session.as_ref().map(|s| s.regions.iter().map(|r| r.id).collect()).unwrap_or_default();
                if ui.add_enabled(has && !all.is_empty(), egui::Button::new("Export All Regions")).clicked() {
                    self.export(&ctx, all);
                }
                if ui.add_enabled(has, egui::Button::new("Export Full Stems")).clicked() {
                    self.export_stems(&ctx);
                }
                if ui.add_enabled(self.session.is_some(), egui::Button::new("Open Export Folder")).clicked()
                    && let Some(s) = &self.session
                {
                    open_folder(&s.export_dir());
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                let dur = self.session.as_ref().map_or(0.0, |s| s.duration);
                if ui.button("Zoom In").clicked() {
                    self.timeline.zoom(1.25, None, dur);
                }
                if ui.button("Zoom Out").clicked() {
                    self.timeline.zoom(0.8, None, dur);
                }
                if ui.button("Whole Song").clicked() {
                    self.timeline.reset(dur);
                }
                ui.separator();
                ui.label("Theme");
                let mut pref = ctx.options(|o| o.theme_preference);
                let before = pref;
                ui.radio_value(&mut pref, egui::ThemePreference::System, "Follow system");
                ui.radio_value(&mut pref, egui::ThemePreference::Light, "Light");
                ui.radio_value(&mut pref, egui::ThemePreference::Dark, "Dark");
                if pref != before {
                    ctx.set_theme(pref);
                    self.prefs.theme = match pref {
                        egui::ThemePreference::Light => "light",
                        egui::ThemePreference::Dark => "dark",
                        egui::ThemePreference::System => "system",
                    }
                    .into();
                }
            });
            ui.menu_button("Tools", |ui| {
                if ui.add_enabled(self.job.is_none(), egui::Button::new("Download Models…")).clicked() {
                    let all: Vec<_> = sw_models::download::ASSETS.iter().filter(|a| sw_models::find_model(a.file).is_err()).collect();
                    if all.is_empty() {
                        self.message("Models", "All models are installed.");
                    } else {
                        self.dialog = Some(Dialog::Models { assets: all, then: None });
                    }
                }
                ui.checkbox(&mut self.prefs.cpu_only, "Separate on the CPU (no GPU)");
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Shortcuts").clicked() {
                    self.message("Shortcuts", SHORTCUTS);
                }
                if ui.button("About").clicked() {
                    self.message(
                        "About SampleWright",
                        format!(
                            "SampleWright {}\n\nSeparate any song into stems and cut loops and one-shots\nfor your DAW, entirely in Rust, on your own GPU.\n\n\
                             Separation: Demucs v4 (Meta) via demucs-rs.\nBeat and bar tracking: Beat This! (JKU Linz).\n\nGPL-3.0-or-later.",
                            env!("CARGO_PKG_VERSION")
                        ),
                    );
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if ui.button("📂 Open").on_hover_text("Open a song (Ctrl+O)").clicked() {
                self.open_dialog(&ctx);
            }
            ui.separator();
            let ready = self.player.ready();
            let playing = self.player.is_playing();
            let label = if self.player.preview_on {
                "⏹ Stop preview"
            } else if playing {
                "⏸ Pause"
            } else {
                "▶ Play"
            };
            if ui.add_enabled(ready, egui::Button::new(label)).on_hover_text("Space").clicked() {
                self.toggle_play();
            }
            if ui.add_enabled(ready, egui::Button::new("⏮")).on_hover_text("Back to the start (Home)").clicked() {
                self.player.seek(0.0);
            }
            if ui.add_enabled(ready, egui::Button::selectable(self.looping, "🔁 Loop")).on_hover_text("Loop playback over the selection or region (L)").clicked() {
                self.looping = !self.looping;
            }
            ui.separator();
            ui.label("Snap");
            egui::ComboBox::from_id_salt("snap").width(64.0).selected_text(self.timeline.snap.label()).show_ui(ui, |ui| {
                for s in Snap::ALL {
                    ui.selectable_value(&mut self.timeline.snap, s, s.label());
                }
            });
            let has_sel = self.timeline.selection.is_some();
            if ui.add_enabled(has_sel, egui::Button::new("+ Region")).on_hover_text("Make a region from the selection (Enter)").clicked() {
                self.add_region();
            }
            ui.separator();
            let mut vol = 1.0f32;
            ui.memory(|m| vol = m.data.get_temp(egui::Id::new("master")).unwrap_or(1.0));
            if ui.add(egui::Slider::new(&mut vol, 0.0..=1.5).show_value(false).text("Volume")).changed() {
                ui.memory_mut(|m| m.data.insert_temp(egui::Id::new("master"), vol));
                self.player.set_master(vol);
            }
            if let Some(s) = &self.session {
                ui.separator();
                let t = self.player.position();
                let bar = s.grid.bar_at(t);
                let info = match s.grid.bpm() {
                    Some(b) => format!("{}   bar {bar}   {b:.1} bpm", fmt_time(t)),
                    None => fmt_time(t),
                };
                ui.label(RichText::new(info).monospace());
            }
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if let Some(job) = &self.job {
                let (f, text) = job.progress.lock().map(|p| p.clone()).unwrap_or_default();
                ui.spinner();
                ui.add(egui::ProgressBar::new(f).desired_width(220.0).show_percentage());
                ui.label(if text.is_empty() { job.label.clone() } else { text });
            } else {
                ui.label(&self.status);
                if let Some(dir) = self.last_export.clone()
                    && ui.small_button("Open folder").clicked()
                {
                    open_folder(&dir);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(g) = self.gpu.lock().ok().and_then(|g| g.clone()) {
                    ui.label(RichText::new(g).weak());
                }
            });
        });
    }

    fn region_tab(&mut self, ui: &mut Ui) {
        let Some(s) = &self.session else { return };
        if s.regions.is_empty() || self.selected.is_none() {
            ui.label(RichText::new("Drag across the lanes to select part of the song (across several lanes to take several stems), then press Enter or + Region.").weak());
            ui.add_space(6.0);
        }
        // the region list
        let mut pick = None;
        if !s.regions.is_empty() {
            ui.label(RichText::new("Regions").strong());
            egui::ScrollArea::vertical().id_salt("regions").max_height(130.0).show(ui, |ui| {
                for r in &s.regions {
                    let text = format!("{}  ·  {}  ·  {}", r.name, r.kind.label(), s.position_label(r).replace('_', " "));
                    if ui.selectable_label(self.selected == Some(r.id), text).clicked() {
                        pick = Some(r.id);
                    }
                }
            });
            ui.separator();
        }
        if let Some(id) = pick {
            self.selected = Some(id);
            if let Some(r) = self.region() {
                let (a, b, d) = (r.start, r.end, s.duration);
                self.timeline.show_range(a, b, d);
            }
        }
        let Some(id) = self.selected else { return };
        let lanes = s.lanes();
        let duration = s.duration;
        let position = self.region().map(|r| s.position_label(r));
        let mut changed = false;
        let mut want_preview = false;
        let mut want_export = false;
        let mut want_delete = false;
        let mut detect = false;
        let mut play_slice = None;
        let previewing = self.previewing.filter(|p| p.0 == id);
        let slices = self.slices.as_ref().filter(|c| c.0 == id).map(|c| c.2.clone());
        let spectrum = self.spectrum.clone();
        let Some(r) = self.session.as_mut().and_then(|s| s.regions.iter_mut().find(|r| r.id == id)) else { return };

        ui.horizontal(|ui| {
            ui.label("Name");
            changed |= ui.add(egui::TextEdit::singleline(&mut r.name).desired_width(f32::INFINITY)).changed();
        });
        ui.horizontal(|ui| {
            for k in Kind::ALL {
                changed |= ui.selectable_value(&mut r.kind, k, k.label()).changed();
            }
        });
        ui.label(
            RichText::new(match r.kind {
                Kind::Loop => "Exact length, crossfaded at the end so it repeats without a click.",
                Kind::OneShot => "One sample with short fades, starting on a zero crossing.",
                Kind::Slices => "One file per note or hit, named by pitch (or kick/snare/hat on drums).",
            })
            .weak(),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Stems");
            for l in &lanes {
                let mut on = r.stems.contains(l);
                let label = if l == sw_core::MIX { "Mix".to_string() } else { capitalise(l) };
                if ui.checkbox(&mut on, RichText::new(label).color(theme::stem_color(l))).changed() {
                    if on {
                        r.stems.push(l.clone());
                        r.stems.sort_by_key(|x| lanes.iter().position(|y| y == x));
                    } else if r.stems.len() > 1 {
                        r.stems.retain(|x| x != l);
                    }
                    changed = true;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("From");
            changed |= ui.add(egui::DragValue::new(&mut r.start).speed(0.01).range(0.0..=r.end - 0.01).suffix(" s").max_decimals(3)).changed();
            ui.label("to");
            changed |= ui.add(egui::DragValue::new(&mut r.end).speed(0.01).range(r.start + 0.01..=duration).suffix(" s").max_decimals(3)).changed();
            ui.label(RichText::new(format!("{:.2} s", r.duration())).weak());
        });
        if let Some(p) = position {
            ui.label(RichText::new(p.replace('_', "  ")).weak());
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let label = if previewing.is_some() { "⏹ Stop preview" } else { "▶ Preview" };
            if ui.button(label).on_hover_text("Hear the region on loop, exactly as it will export (P)").clicked() {
                want_preview = true;
            }
            if ui.button(RichText::new("⬇ Export").color(theme::accent_text(ui.ctx()))).on_hover_text("Ctrl+E").clicked() {
                want_export = true;
            }
            if ui.button("🗑").on_hover_text("Delete the region (Delete)").clicked() {
                want_delete = true;
            }
        });
        ui.add_space(4.0);

        let c = &mut r.chain;
        egui::CollapsingHeader::new(RichText::new("Filters").strong()).default_open(true).show(ui, |ui| {
            if filter_view::ui(ui, &mut c.filters, spectrum.as_ref().map(|s| &s.1), sw_core::SAMPLE_RATE) {
                changed = true;
            }
            ui.label(RichText::new("Drag the handles; scroll over a band handle to make it wider or narrower.").weak().small());
            let f = &mut c.filters;
            for (pass, name) in [(&mut f.highpass, "High-pass"), (&mut f.lowpass, "Low-pass")] {
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut pass.on, name).changed();
                    ui.add_enabled_ui(pass.on, |ui| {
                        let speed = pass.freq * 0.005;
                        changed |= ui.add(egui::DragValue::new(&mut pass.freq).range(20.0..=20_000.0).speed(speed).suffix(" Hz").max_decimals(0)).changed();
                        egui::ComboBox::from_id_salt(name).width(90.0).selected_text(pass.slope.label()).show_ui(ui, |ui| {
                            for sl in Slope::ALL {
                                changed |= ui.selectable_value(&mut pass.slope, sl, sl.label()).changed();
                            }
                        });
                    });
                });
            }
            for (i, b) in f.bands.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut b.on, format!("Band {}", i + 1)).changed();
                    ui.add_enabled_ui(b.on, |ui| {
                        egui::ComboBox::from_id_salt(("band", i)).width(64.0).selected_text(if b.kind == BandKind::Bell { "Bell" } else { "Notch" }).show_ui(ui, |ui| {
                            changed |= ui.selectable_value(&mut b.kind, BandKind::Bell, "Bell").changed();
                            changed |= ui.selectable_value(&mut b.kind, BandKind::Notch, "Notch").changed();
                        });
                        let speed = b.freq * 0.005;
                        changed |= ui.add(egui::DragValue::new(&mut b.freq).range(20.0..=20_000.0).speed(speed).suffix(" Hz").max_decimals(0)).changed();
                        if b.kind == BandKind::Bell {
                            changed |= ui.add(egui::DragValue::new(&mut b.gain_db).range(-24.0..=12.0).speed(0.1).suffix(" dB").max_decimals(1)).changed();
                        }
                        changed |= ui.add(egui::DragValue::new(&mut b.q).range(0.2..=30.0).speed(0.02).prefix("Q ").max_decimals(2)).changed();
                    });
                });
            }
        });
        egui::CollapsingHeader::new(RichText::new("Noise gate").strong()).show(ui, |ui| {
            ui.label(RichText::new("Silences the quiet bleed between notes. Opens a few ms early, so attacks are kept.").weak().small());
            let g = &mut c.gate;
            changed |= ui.checkbox(&mut g.on, "On").changed();
            ui.add_enabled_ui(g.on, |ui| {
                egui::Grid::new("gate").num_columns(2).show(ui, |ui| {
                    ui.label("Threshold");
                    changed |= ui.add(egui::Slider::new(&mut g.threshold_db, -80.0..=0.0).suffix(" dB")).changed();
                    ui.end_row();
                    ui.label("Reduction");
                    changed |= ui.add(egui::Slider::new(&mut g.range_db, 0.0..=80.0).suffix(" dB")).changed();
                    ui.end_row();
                    ui.label("Attack");
                    changed |= ui.add(egui::Slider::new(&mut g.attack_ms, 0.1..=50.0).logarithmic(true).suffix(" ms")).changed();
                    ui.end_row();
                    ui.label("Hold");
                    changed |= ui.add(egui::Slider::new(&mut g.hold_ms, 0.0..=500.0).suffix(" ms")).changed();
                    ui.end_row();
                    ui.label("Release");
                    changed |= ui.add(egui::Slider::new(&mut g.release_ms, 1.0..=1000.0).logarithmic(true).suffix(" ms")).changed();
                    ui.end_row();
                });
            });
        });
        egui::CollapsingHeader::new(RichText::new("Level and fades").strong()).show(ui, |ui| {
            egui::Grid::new("level").num_columns(2).show(ui, |ui| {
                ui.label("Gain");
                changed |= ui.add(egui::Slider::new(&mut c.gain_db, -24.0..=24.0).suffix(" dB")).changed();
                ui.end_row();
                changed |= ui.checkbox(&mut c.normalize, "Normalise to").changed();
                changed |= ui.add_enabled(c.normalize, egui::Slider::new(&mut c.normalize_db, -24.0..=0.0).suffix(" dBFS")).changed();
                ui.end_row();
                if r.kind == Kind::Loop {
                    ui.label("Loop crossfade");
                    changed |= ui.add(egui::Slider::new(&mut c.loop_xfade_ms, 0.0..=200.0).suffix(" ms")).changed();
                    ui.end_row();
                } else {
                    ui.label("Fade in");
                    changed |= ui.add(egui::Slider::new(&mut c.fade_in_ms, 0.0..=100.0).suffix(" ms")).changed();
                    ui.end_row();
                    ui.label("Fade out");
                    changed |= ui.add(egui::Slider::new(&mut c.fade_out_ms, 0.0..=500.0).suffix(" ms")).changed();
                    ui.end_row();
                }
            });
        });
        if r.kind == Kind::Slices {
            egui::CollapsingHeader::new(RichText::new("Slicing").strong()).default_open(true).show(ui, |ui| {
                let sl = &mut r.slicing;
                egui::Grid::new("slicing").num_columns(2).show(ui, |ui| {
                    ui.label("Sensitivity");
                    detect |= ui.add(egui::Slider::new(&mut sl.sensitivity, 0.0..=1.0)).changed();
                    ui.end_row();
                    ui.label("Min. gap");
                    detect |= ui.add(egui::Slider::new(&mut sl.min_gap_ms, 10.0..=500.0).suffix(" ms")).changed();
                    ui.end_row();
                    ui.label("End when");
                    detect |= ui.add(egui::Slider::new(&mut sl.decay_db, 6.0..=80.0).prefix("-").suffix(" dB")).changed();
                    ui.end_row();
                    ui.label("Max. length");
                    detect |= ui.add(egui::Slider::new(&mut sl.max_len_ms, 100.0..=10_000.0).logarithmic(true).suffix(" ms")).changed();
                    ui.end_row();
                });
                match &slices {
                    Some(cuts) => {
                        ui.label(format!("{} slices", cuts.len()));
                        egui::ScrollArea::vertical().id_salt("slices").max_height(200.0).show(ui, |ui| {
                            for (i, cut) in cuts.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    let on = previewing == Some((id, Some(i)));
                                    if ui.small_button(if on { "⏹" } else { "▶" }).clicked() {
                                        play_slice = Some((i, on));
                                    }
                                    ui.label(format!("{:02}  {}  {:.0} ms", i + 1, cut.label.as_deref().unwrap_or("·"), (cut.end - cut.start) * 1000.0));
                                });
                            }
                        });
                    }
                    None => {
                        ui.label(RichText::new("Finding notes…").weak());
                    }
                }
            });
        }
        if changed || detect {
            self.changed();
        }
        if want_preview {
            if previewing.is_some() {
                self.stop_preview();
            } else {
                self.preview(id, None);
            }
        }
        if let Some((i, on)) = play_slice {
            if on {
                self.stop_preview();
            } else {
                self.preview(id, Some(i));
            }
        }
        if want_export {
            self.export(&ui.ctx().clone(), vec![id]);
        }
        if want_delete {
            self.delete_region();
        }
    }

    fn song_tab(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let job_free = self.job.is_none();
        let Some(s) = &mut self.session else { return };
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Title");
            changed |= ui.add(egui::TextEdit::singleline(&mut s.title).desired_width(f32::INFINITY)).on_hover_text("Used in exported file names").changed();
        });
        egui::Grid::new("song").num_columns(2).show(ui, |ui| {
            ui.label(RichText::new("File").weak());
            ui.label(s.source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).on_hover_text(s.source.display().to_string());
            ui.end_row();
            ui.label(RichText::new("Length").weak());
            ui.label(fmt_time(s.duration));
            ui.end_row();
            ui.label(RichText::new("Tempo").weak());
            ui.label(match (s.grid.bpm(), s.grid.beats_per_bar()) {
                (Some(b), Some(n)) => format!("{b:.1} bpm, {n}/4, {} bars", s.grid.downbeats.len()),
                _ => "no steady beat found".into(),
            });
            ui.end_row();
            ui.label(RichText::new("Stems").weak());
            ui.label(format!("{} ({})", s.model, if s.device.is_empty() { "?" } else { &s.device }));
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.label(RichText::new("Separation").strong());
        let mut model = s.model.clone();
        let done = s.separated_models();
        for (m, label, what) in MODELS {
            let tick = if done.iter().any(|d| d == m) { "  ✔" } else { "" };
            ui.radio_value(&mut model, m.to_string(), format!("{label}{tick}")).on_hover_text(what);
        }
        let mut separate_with = None;
        if model != s.model {
            let verb = if done.contains(&model) { "Switch to these stems" } else { "Separate with this model" };
            if ui.add_enabled(job_free, egui::Button::new(verb)).clicked() {
                separate_with = Some((s.source.clone(), model.clone()));
            }
            if !s.source.is_file() {
                ui.label(RichText::new("The song file has moved, so it can't be separated again.").weak());
            }
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Export").strong());
        let e = &mut s.export;
        egui::Grid::new("export").num_columns(2).show(ui, |ui| {
            ui.label("Format");
            egui::ComboBox::from_id_salt("fmt").selected_text(e.format.label()).show_ui(ui, |ui| {
                for f in Format::ALL {
                    changed |= ui.selectable_value(&mut e.format, f, f.label()).changed();
                }
            });
            ui.end_row();
            ui.label("Sample rate");
            egui::ComboBox::from_id_salt("rate").selected_text(format!("{} Hz", e.sample_rate)).show_ui(ui, |ui| {
                for r in [44_100, 48_000] {
                    changed |= ui.selectable_value(&mut e.sample_rate, r, format!("{r} Hz")).changed();
                }
            });
            ui.end_row();
            ui.label("Channels");
            ui.horizontal(|ui| {
                changed |= ui.radio_value(&mut e.mono, false, "Stereo").changed();
                changed |= ui.radio_value(&mut e.mono, true, "Mono").changed();
            });
            ui.end_row();
        });
        changed |= ui.checkbox(&mut e.separate_stems, "One file per stem for multi-stem regions").changed();
        let dir = s.export_dir();
        ui.horizontal(|ui| {
            ui.label("Folder");
            ui.label(RichText::new(dir.display().to_string()).monospace().small());
        });
        ui.horizontal(|ui| {
            if ui.button("Choose…").clicked()
                && let Some(d) = rfd::FileDialog::new().set_directory(&dir).pick_folder()
            {
                s.export.dir = Some(d);
                changed = true;
            }
            if s.export.dir.is_some() && ui.button("Default").clicked() {
                s.export.dir = None;
                changed = true;
            }
            if ui.button("Open").clicked() {
                open_folder(&dir);
            }
        });
        ui.add_space(4.0);
        let all: Vec<u64> = s.regions.iter().map(|r| r.id).collect();
        let loaded = self.stems.is_some();
        let mut export_all = false;
        let mut export_stems = false;
        ui.horizontal(|ui| {
            if ui.add_enabled(loaded && job_free && !all.is_empty(), egui::Button::new(format!("Export all {} regions", all.len()))).clicked() {
                export_all = true;
            }
            if ui.add_enabled(loaded && job_free, egui::Button::new("Export full stems")).clicked() {
                export_stems = true;
            }
        });
        if changed {
            self.changed();
        }
        if let Some((song, model)) = separate_with {
            self.prefs.model = model.clone();
            self.run_separation(&ctx, song, model);
        }
        if export_all {
            self.export(&ctx, all);
        }
        if export_stems {
            self.export_stems(&ctx);
        }
    }

    /// Keeps the slice list and the spectrum of the selected region current.
    fn refresh_analysis(&mut self) {
        let (Some(s), Some(stems), Some(id)) = (&self.session, &self.stems, self.selected) else { return };
        let Some(r) = s.regions.iter().find(|r| r.id == id) else { return };
        let dragging = self.timeline.dragging();
        let key = hash_of(&(r.start, r.end, &r.stems, &r.chain.filters, &r.chain.gate, r.chain.gain_db));
        if !dragging && self.spectrum.as_ref().is_none_or(|sp| sp.0 != key) {
            // the unprocessed input, so the curve shows what the filters will cut
            let raw = Region::new(0, r.start, r.end, r.stems.clone());
            let x = raw.process(stems, &r.stems);
            let body = x.audio.slice(x.pre, x.pre + x.len).mono();
            self.spectrum = Some((key, (sw_audio::spectrum::average_db(&body, 120), sw_core::SAMPLE_RATE)));
        }
        if r.kind == Kind::Slices {
            let key = hash_of(&(r.start, r.end, &r.stems, &r.chain.filters, &r.chain.gate, r.chain.gain_db, &r.slicing));
            if !dragging && self.slices.as_ref().is_none_or(|c| c.0 != id || c.1 != key) {
                let cuts = slice::detect(r, stems);
                self.slices = Some((id, key, cuts));
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else { return };
        let modal = |id: &str| egui::Modal::new(egui::Id::new(id));
        match dialog {
            Dialog::Message(title, text) => {
                let r = modal("msg").show(ctx, |ui| {
                    ui.set_max_width(560.0);
                    ui.heading(&title);
                    ui.add_space(6.0);
                    ui.label(RichText::new(&text).monospace());
                    ui.add_space(6.0);
                    ui.button("OK").clicked()
                });
                if !(r.inner || r.should_close()) {
                    self.dialog = Some(Dialog::Message(title, text));
                }
            }
            Dialog::Separate { song, mut model } => {
                let mut go = false;
                let mut keep = true;
                let r = modal("separate").show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading("Separate the song");
                    ui.label(song.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
                    ui.add_space(6.0);
                    for (m, label, what) in MODELS {
                        ui.radio_value(&mut model, m.to_string(), RichText::new(label).strong());
                        ui.label(RichText::new(what).weak());
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("This runs once per song and model; the stems are kept, so the song opens instantly next time. A few minutes' song takes about a minute on a GPU.")
                            .weak(),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Separate").color(theme::accent_text(ui.ctx()))).clicked() {
                            go = true;
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                if go {
                    self.prefs.model = model.clone();
                    self.run_separation(ctx, song, model);
                } else if keep && !r.should_close() {
                    self.dialog = Some(Dialog::Separate { song, model });
                }
            }
            Dialog::Models { assets, then } => {
                let mut start = false;
                let mut keep = true;
                let r = modal("models").show(ctx, |ui| {
                    ui.set_width(540.0);
                    ui.heading("Download AI models");
                    ui.label("SampleWright uses neural networks to separate the stems and find the beats. These files are needed:");
                    ui.add_space(4.0);
                    for a in &assets {
                        ui.label(format!("• {}  ({:.0} MB)", a.what, a.size as f64 / 1e6));
                    }
                    let total: u64 = assets.iter().map(|a| a.size).sum();
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!(
                            "{:.0} MB in total, checked by checksum, saved to {}. Everything runs on your computer: no audio is uploaded.",
                            total as f64 / 1e6,
                            sw_models::download::install_dir().map(|d| d.display().to_string()).unwrap_or_default()
                        ))
                        .weak(),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Download").color(theme::accent_text(ui.ctx()))).clicked() {
                            start = true;
                        }
                        if ui.button("Not now").clicked() {
                            keep = false;
                        }
                    });
                });
                if start {
                    self.download(ctx, assets, then);
                } else if keep && !r.should_close() {
                    self.dialog = Some(Dialog::Models { assets, then });
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job(&ctx);
        self.player.poll();

        // dropped files
        let dropped: Option<PathBuf> = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(p) = dropped {
            self.open_song(&ctx, &p);
        }
        if self.dialog.is_none() {
            self.shortcuts(&ctx);
        }
        self.update_gains();
        self.update_loop();
        self.refresh_analysis();
        if let Some(due) = self.preview_due
            && Instant::now() >= due
        {
            self.preview_due = None;
            if let Some((id, slice)) = self.previewing {
                self.preview(id, slice);
            }
        }
        if self.dirty.is_some_and(|t| t.elapsed() > Duration::from_secs(1)) {
            self.save_now();
        }
        let playing = self.player.is_playing();
        let dur = self.session.as_ref().map_or(0.0, |s| s.duration);
        let playhead = self.player.position();
        if playing {
            self.timeline.follow(playhead, dur);
            ctx.request_repaint();
        } else if self.job.is_some() || self.preview_due.is_some() || self.dirty.is_some() || !self.player.ready() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let title = match &self.session {
            Some(s) => format!("SampleWright - {}", s.title),
            None => "SampleWright".into(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        egui::Panel::top("menu").show(ui, |ui| self.menus(ui));
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if self.session.is_some() {
            egui::Panel::right("side").default_size(400.0).min_size(340.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Region, "Region");
                    ui.selectable_value(&mut self.tab, Tab::Song, "Song and export");
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                    Tab::Region => self.region_tab(ui),
                    Tab::Song => self.song_tab(ui),
                });
            });
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let busy = self.job.as_ref().map(|j| j.label.clone());
            let slices = self.slices.as_ref().map(|c| (c.0, c.2.as_slice()));
            let session = if self.stems.is_some() { self.session.as_mut() } else { None };
            let ev = self.timeline.ui(ui, session, &self.lanes, self.selected, playhead, slices, busy.as_deref());
            if let Some(t) = ev.seek {
                if self.player.preview_on {
                    self.stop_preview();
                }
                self.player.seek(t);
            }
            if let Some(sel) = ev.select_region {
                self.selected = sel;
                if sel.is_some() {
                    self.tab = Tab::Region;
                }
            }
            if ev.region_changed {
                self.changed();
            }
            if let Some(i) = ev.toggle_mute
                && let Some(l) = self.lanes.get_mut(i)
            {
                l.mute = !l.mute;
            }
            if let Some(i) = ev.toggle_solo
                && let Some(l) = self.lanes.get_mut(i)
            {
                l.solo = !l.solo;
            }
            if let Some(id) = ev.preview_region {
                self.preview(id, None);
            }
        });
        self.dialogs(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "prefs", &self.prefs);
        self.save_now();
    }
}
