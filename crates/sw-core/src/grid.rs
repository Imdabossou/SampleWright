//! The beat grid: beats and bar lines from Beat This!, the tempo, and
//! snapping selections to them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    /// Beat times in seconds.
    pub beats: Vec<f64>,
    /// Bar starts (downbeats) in seconds.
    pub downbeats: Vec<f64>,
}

/// What a selection edge snaps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Snap {
    Off,
    Beat,
    #[default]
    Bar,
}

impl Snap {
    pub const ALL: [Snap; 3] = [Snap::Off, Snap::Beat, Snap::Bar];

    pub fn label(self) -> &'static str {
        match self {
            Snap::Off => "Off",
            Snap::Beat => "Beat",
            Snap::Bar => "Bar",
        }
    }
}

impl Grid {
    /// Tempo from the median beat interval (robust to the odd missed beat).
    pub fn bpm(&self) -> Option<f64> {
        let mut d: Vec<f64> = self.beats.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.2 && *d < 2.0).collect();
        if d.len() < 4 {
            return None;
        }
        d.sort_by(f64::total_cmp);
        Some(60.0 / d[d.len() / 2])
    }

    /// Beats per bar, the most common count between downbeats.
    pub fn beats_per_bar(&self) -> Option<usize> {
        let mut counts = [0usize; 13];
        for w in self.downbeats.windows(2) {
            let n = self.beats.iter().filter(|&&b| b >= w[0] - 1e-3 && b < w[1] - 1e-3).count();
            if n < counts.len() {
                counts[n] += 1;
            }
        }
        counts.iter().enumerate().skip(1).max_by_key(|(_, c)| **c).filter(|(_, c)| **c > 0).map(|(n, _)| n)
    }

    /// 1-based bar number at `t` (0 before the first bar line).
    pub fn bar_at(&self, t: f64) -> usize {
        self.downbeats.partition_point(|&d| d <= t + 1e-6)
    }

    /// Bars covered by `start..end`, if both edges sit on bar lines.
    pub fn bars_of(&self, start: f64, end: f64) -> Option<(usize, usize)> {
        let on = |t: f64| self.downbeats.iter().any(|d| (d - t).abs() < 0.005) || self.downbeats.last().is_some_and(|l| t > *l && self.bar_end(*l).is_some_and(|e| (e - t).abs() < 0.005));
        if !(on(start) && on(end)) {
            return None;
        }
        let a = self.bar_at(start);
        let b = self.bar_at(end - 0.01);
        (a >= 1 && b >= a).then_some((a, b))
    }

    /// Where the bar starting at `downbeat` ends (the next downbeat, or one
    /// average bar later for the last one).
    fn bar_end(&self, downbeat: f64) -> Option<f64> {
        let i = self.downbeats.iter().position(|d| (*d - downbeat).abs() < 1e-9)?;
        self.downbeats.get(i + 1).copied().or_else(|| {
            let n = self.downbeats.len();
            (n >= 2).then(|| downbeat + (self.downbeats[n - 1] - self.downbeats[0]) / (n - 1) as f64)
        })
    }

    /// The nearest grid line to `t` (or `t` itself with no grid).
    pub fn snap(&self, t: f64, snap: Snap) -> f64 {
        let lines: &[f64] = match snap {
            Snap::Off => return t,
            Snap::Beat => &self.beats,
            Snap::Bar => &self.downbeats,
        };
        let mut best = t;
        let mut dist = f64::INFINITY;
        let i = lines.partition_point(|&l| l < t);
        for &l in lines[i.saturating_sub(1)..(i + 1).min(lines.len())].iter() {
            if (l - t).abs() < dist {
                dist = (l - t).abs();
                best = l;
            }
        }
        // past the last bar line: the end of the last bar
        if snap == Snap::Bar
            && let Some(&last) = lines.last()
            && let Some(e) = self.bar_end(last)
            && (e - t).abs() < dist
        {
            best = e;
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Grid {
        // 120 bpm in 4/4 from 0.5 s
        let beats: Vec<f64> = (0..64).map(|i| 0.5 + i as f64 * 0.5).collect();
        let downbeats = beats.iter().step_by(4).copied().collect();
        Grid { beats, downbeats }
    }

    #[test]
    fn tempo_bars_and_snapping() {
        let g = grid();
        assert!((g.bpm().unwrap() - 120.0).abs() < 1e-9);
        assert_eq!(g.beats_per_bar(), Some(4));
        assert_eq!(g.bar_at(0.2), 0);
        assert_eq!(g.bar_at(0.5), 1);
        assert_eq!(g.bar_at(2.6), 2);
        assert_eq!(g.snap(2.4, Snap::Bar), 2.5);
        assert_eq!(g.snap(2.8, Snap::Beat), 3.0);
        assert_eq!(g.snap(2.8, Snap::Off), 2.8);
        // bars 2..3 are 2.5..6.5
        assert_eq!(g.bars_of(2.5, 6.5), Some((2, 3)));
        assert_eq!(g.bars_of(2.5, 6.0), None);
        // the last bar ends one bar after its downbeat
        let last = *g.downbeats.last().unwrap();
        assert_eq!(g.snap(last + 1.9, Snap::Bar), last + 2.0);
        assert_eq!(g.bars_of(last, last + 2.0), Some((16, 16)));
    }
}
