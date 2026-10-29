//! Playback: the song's lanes mixed live (so mute, solo and volume act at
//! once), optionally looping a stretch, or a processed region previewed on
//! loop exactly as it will export.
//!
//! Lanes are resampled to the output device's rate once, in the background;
//! the cpal callback only mixes.

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sw_audio::Audio;

/// Interleaved stereo at the device rate.
type Buf = Arc<Vec<f32>>;

struct Shared {
    lanes: Vec<Buf>,
    gains: Vec<f32>,
    /// Song position in frames.
    pos: usize,
    playing: bool,
    /// Loop of the song, in frames.
    looping: Option<(usize, usize)>,
    /// A rendered region playing on loop instead of the song.
    preview: Option<Buf>,
    preview_pos: usize,
    master: f32,
}

pub struct Player {
    _stream: Option<cpal::Stream>,
    shared: Arc<Mutex<Shared>>,
    pub rate: u32,
    pub error: Option<String>,
    loading: Option<Receiver<Vec<Buf>>>,
    rendering: Option<Receiver<(u64, Buf)>>,
    /// Song time the preview starts at, for the playhead.
    preview_start: f64,
    /// Generation of the preview being rendered, so a stale render is dropped.
    preview_gen: u64,
    pub preview_on: bool,
    frames: usize,
}

impl Player {
    pub fn new() -> Player {
        let shared = Arc::new(Mutex::new(Shared {
            lanes: Vec::new(),
            gains: Vec::new(),
            pos: 0,
            playing: false,
            looping: None,
            preview: None,
            preview_pos: 0,
            master: 1.0,
        }));
        let (stream, rate, error) = match open_stream(shared.clone()) {
            Ok((s, r)) => (Some(s), r, None),
            Err(e) => (None, 48_000, Some(e)),
        };
        Player {
            _stream: stream,
            shared,
            rate,
            error,
            loading: None,
            rendering: None,
            preview_start: 0.0,
            preview_gen: 0,
            preview_on: false,
            frames: 0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn interleave(a: &Audio, rate: u32) -> Vec<f32> {
        let a = if a.sample_rate == rate { a.clone() } else { a.resample(rate) };
        let (l, r) = (&a.channels[0], a.channels.get(1).unwrap_or(&a.channels[0]));
        l.iter().zip(r).flat_map(|(l, r)| [*l, *r]).collect()
    }

    /// Starts preparing the lanes (in display order) for playback.
    pub fn load(&mut self, lanes: Vec<Arc<Audio>>) {
        self.stop();
        {
            let mut s = self.lock();
            s.lanes.clear();
            s.gains.clear();
            s.pos = 0;
        }
        let rate = self.rate;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            use rayon::prelude::*;
            let bufs = lanes.par_iter().map(|a| Arc::new(Self::interleave(a, rate))).collect();
            let _ = tx.send(bufs);
        });
        self.loading = Some(rx);
    }

    pub fn ready(&self) -> bool {
        self.loading.is_none() && self.frames > 0
    }

    /// Call every frame: picks up finished background work.
    pub fn poll(&mut self) {
        if let Some(rx) = &self.loading
            && let Ok(bufs) = rx.try_recv()
        {
            self.loading = None;
            self.frames = bufs.iter().map(|b| b.len() / 2).max().unwrap_or(0);
            let mut s = self.lock();
            s.gains = vec![1.0; bufs.len()];
            s.lanes = bufs;
        }
        if let Some(rx) = &self.rendering
            && let Ok((generation, buf)) = rx.try_recv()
        {
            self.rendering = None;
            if generation == self.preview_gen && self.preview_on {
                let mut s = self.lock();
                // keep the position when the chain changed mid-loop
                s.preview_pos = if buf.is_empty() { 0 } else { s.preview_pos % (buf.len() / 2).max(1) };
                s.preview = Some(buf);
                s.playing = true;
            }
        }
    }

    pub fn set_gains(&mut self, gains: Vec<f32>) {
        let mut s = self.lock();
        if s.gains.len() == gains.len() {
            s.gains = gains;
        }
    }

    pub fn set_master(&mut self, v: f32) {
        self.lock().master = v;
    }

    pub fn is_playing(&self) -> bool {
        self.lock().playing
    }

    pub fn play(&mut self) {
        if self.ready() {
            let mut s = self.lock();
            if s.pos >= self.frames {
                s.pos = s.looping.map_or(0, |l| l.0);
            }
            s.playing = true;
        }
    }

    pub fn pause(&mut self) {
        self.lock().playing = false;
    }

    pub fn stop(&mut self) {
        self.stop_preview();
        self.lock().playing = false;
    }

    pub fn seek(&mut self, t: f64) {
        let f = (t.max(0.0) * self.rate as f64) as usize;
        let mut s = self.lock();
        s.pos = f.min(self.frames);
    }

    /// Loop the song between two times (or not).
    pub fn set_loop(&mut self, range: Option<(f64, f64)>) {
        let rate = self.rate as f64;
        let mut s = self.lock();
        s.looping = range.map(|(a, b)| ((a.max(0.0) * rate) as usize, (b.max(0.0) * rate) as usize)).filter(|(a, b)| b > a);
        if let Some((a, b)) = s.looping
            && (s.pos < a || s.pos >= b)
        {
            s.pos = a;
        }
    }

    /// Plays `audio` (a rendered region starting at song time `start`) on loop,
    /// replacing the previous preview once it is ready.
    pub fn preview(&mut self, audio: Audio, start: f64) {
        self.preview_on = true;
        self.preview_start = start;
        self.preview_gen += 1;
        let (generation, rate) = (self.preview_gen, self.rate);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send((generation, Arc::new(Self::interleave(&audio, rate))));
        });
        self.rendering = Some(rx);
    }

    pub fn stop_preview(&mut self) {
        if self.preview_on {
            self.preview_on = false;
            self.rendering = None;
            let mut s = self.lock();
            s.preview = None;
            s.preview_pos = 0;
            s.playing = false;
        }
    }

    /// The playhead in song seconds.
    pub fn position(&self) -> f64 {
        let s = self.lock();
        let rate = self.rate as f64;
        match &s.preview {
            Some(_) if self.preview_on => self.preview_start + s.preview_pos as f64 / rate,
            _ => s.pos as f64 / rate,
        }
    }
}

fn open_stream(shared: Arc<Mutex<Shared>>) -> Result<(cpal::Stream, u32), String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no audio output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate();
    let channels = config.channels() as usize;
    let format = config.sample_format();
    let config: cpal::StreamConfig = config.into();
    let err = |e| eprintln!("audio stream error: {e}");

    macro_rules! build {
        ($t:ty) => {
            device.build_output_stream(
                config.clone(),
                move |data: &mut [$t], _| {
                    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    let master = s.master;
                    for frame in data.chunks_mut(channels) {
                        let (mut l, mut r) = (0f32, 0f32);
                        if s.playing {
                            if let Some(p) = s.preview.clone() {
                                let n = p.len() / 2;
                                if n > 0 {
                                    let i = s.preview_pos % n;
                                    l = p[2 * i];
                                    r = p[2 * i + 1];
                                    s.preview_pos = (i + 1) % n;
                                }
                            } else {
                                let i = s.pos;
                                for (lane, g) in s.lanes.iter().zip(&s.gains) {
                                    if *g > 0.0 && 2 * i + 1 < lane.len() {
                                        l += lane[2 * i] * g;
                                        r += lane[2 * i + 1] * g;
                                    }
                                }
                                s.pos += 1;
                                let end = s.lanes.iter().map(|b| b.len() / 2).max().unwrap_or(0);
                                match s.looping {
                                    Some((a, b)) if s.pos >= b => s.pos = a,
                                    _ if s.pos >= end => s.playing = false,
                                    _ => {}
                                }
                            }
                        }
                        let (l, r) = ((l * master).clamp(-1.0, 1.0), (r * master).clamp(-1.0, 1.0));
                        for (c, out) in frame.iter_mut().enumerate() {
                            let v = match (channels, c) {
                                (1, _) => (l + r) * 0.5,
                                (_, 0) => l,
                                (_, 1) => r,
                                _ => 0.0,
                            };
                            *out = <$t as cpal::FromSample<f32>>::from_sample_(v);
                        }
                    }
                },
                err,
                None,
            )
        };
    }
    let stream = match format {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::U16 => build!(u16),
        other => return Err(format!("unsupported audio sample format {other}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, rate))
}
