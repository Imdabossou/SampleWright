//! The app icon, drawn in code so every size (window, .desktop, .ico, .icns)
//! comes from one source: a dark rounded tile with an amber frame, four stem
//! waveforms in their lane colours, and a selection bracket lifting one
//! section out of them.

use crate::theme::stem_color;

fn rounded_rect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r && x >= x0 && x <= x1 && y >= y0 && y <= y1
}

/// Height (0..1) of the waveform bar in column `col` of a lane: a pseudo-random
/// level shaped by a rhythm, so each stem has its own pattern.
fn bar(lane: usize, col: usize) -> f32 {
    let mut h = (col as u32).wrapping_mul(0x9E37_79B9) ^ (lane as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    let noise = (h & 0xffff) as f32 / 65_535.0;
    let period = [4, 6, 3, 8][lane];
    let accent = if col.is_multiple_of(period) { 1.0 } else { 0.55 };
    (0.25 + 0.75 * noise) * accent
}

/// RGBA pixels of the icon at `size` x `size`.
pub fn rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4; // supersampling per axis
    let s = size as f32;
    let lanes = ["drums", "bass", "guitar", "piano"];
    let accent = [0xf0 as f32, 0xa0 as f32, 0x30 as f32];
    let (sel0, sel1) = (0.42, 0.70);
    let mut out = vec![0u8; (size * size * 4) as usize];
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) / s;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / s;
                    let mut c: Option<[f32; 3]> = None;
                    if rounded_rect(x, y, 0.03, 0.03, 0.97, 0.97, 0.18) {
                        c = Some([27.0, 29.0, 35.0]);
                        if !rounded_rect(x, y, 0.07, 0.07, 0.93, 0.93, 0.14) {
                            c = Some(accent);
                        }
                        let inside = x > sel0 && x < sel1;
                        // the selection: a lighter band behind the waveforms
                        if inside && y > 0.17 && y < 0.83 {
                            c = Some([52.0, 48.0, 40.0]);
                        }
                        // waveform bars: columns 0.025 wide with a gap
                        let (u, cols) = ((x - 0.13) / 0.025, 30.0);
                        let in_bar = u >= 0.0 && u < cols && u.fract() < 0.6;
                        for (k, name) in lanes.iter().enumerate() {
                            let cy = 0.255 + k as f32 * 0.163;
                            let h = 0.07 * bar(k, u.max(0.0) as usize);
                            if in_bar && (y - cy).abs() < h.max(0.008) {
                                let col = stem_color(name);
                                let dim = if inside { 1.0 } else { 0.55 };
                                c = Some([col.r() as f32 * dim, col.g() as f32 * dim, col.b() as f32 * dim]);
                            }
                        }
                        // bracket edges with small serifs
                        let edge = |ex: f32| (x - ex).abs() < 0.014 && y > 0.15 && y < 0.85;
                        let serif = |ex: f32, dir: f32| {
                            let dx = (x - ex) * dir;
                            (-0.014..0.06).contains(&dx) && ((y - 0.15).abs() < 0.014 || (y - 0.85).abs() < 0.014)
                        };
                        if edge(sel0) || edge(sel1) || serif(sel0, 1.0) || serif(sel1, -1.0) {
                            c = Some(accent);
                        }
                    }
                    if let Some(c) = c {
                        acc[0] += c[0];
                        acc[1] += c[1];
                        acc[2] += c[2];
                        acc[3] += 255.0;
                    }
                }
            }
            let n = (SS * SS) as f32;
            let i = ((py * size + px) * 4) as usize;
            let a = acc[3] / n;
            // un-premultiply the averaged colour
            for k in 0..3 {
                out[i + k] = if a > 0.0 { (acc[k] / n * 255.0 / a).round() as u8 } else { 0 };
            }
            out[i + 3] = a.round() as u8;
        }
    }
    out
}

pub fn icon_data(size: u32) -> eframe::egui::IconData {
    eframe::egui::IconData { rgba: rgba(size), width: size, height: size }
}

/// Writes the icon for packaging (`samplewright --write-icon <file> [size]`):
/// a PNG at `size`, or a Windows .ico with the standard sizes.
pub fn write(path: &std::path::Path, size: u32) -> Result<(), String> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ico")) {
        let frames = [16u32, 24, 32, 48, 64, 128, 256]
            .iter()
            .map(|&s| image::codecs::ico::IcoFrame::as_png(&rgba(s), s, s, image::ExtendedColorType::Rgba8))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        return image::codecs::ico::IcoEncoder::new(file).encode_images(&frames).map_err(|e| e.to_string());
    }
    image::RgbaImage::from_raw(size, size, rgba(size)).ok_or("bad icon size")?.save(path).map_err(|e| e.to_string())
}
