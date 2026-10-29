//! Waveform overviews: min/max pairs at several block sizes, so any zoom
//! level draws from a few thousand values instead of millions of samples.

#[derive(Debug, Clone, Default)]
pub struct Peaks {
    /// (block size in frames, min/max per block) from finest to coarsest.
    levels: Vec<(usize, Vec<(f32, f32)>)>,
    frames: usize,
}

const FINEST: usize = 32;

impl Peaks {
    /// Overview of a mono (or summed) signal.
    pub fn new(x: &[f32]) -> Peaks {
        let mut levels = Vec::new();
        let first: Vec<(f32, f32)> =
            x.chunks(FINEST).map(|c| c.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)))).collect();
        levels.push((FINEST, first));
        while levels.last().is_some_and(|l| l.1.len() > 64) {
            let (bs, prev) = levels.last().expect("non-empty");
            let next = prev.chunks(4).map(|c| c.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &(a, b)| (lo.min(a), hi.max(b)))).collect();
            levels.push((bs * 4, next));
        }
        Peaks { levels, frames: x.len() }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Min and max of frames `from..to`, from the coarsest level that still
    /// resolves the range.
    pub fn range(&self, from: usize, to: usize) -> (f32, f32) {
        let to = to.min(self.frames);
        if from >= to {
            return (0.0, 0.0);
        }
        let span = to - from;
        let level = self.levels.iter().rev().find(|(bs, _)| *bs * 2 <= span).or(self.levels.first());
        let Some((bs, data)) = level else { return (0.0, 0.0) };
        let (a, b) = (from / bs, to.div_ceil(*bs).min(data.len()));
        if a >= b {
            return (0.0, 0.0);
        }
        data[a..b].iter().fold((f32::MAX, f32::MIN), |(lo, hi), &(x, y)| (lo.min(x), hi.max(y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_cover_their_samples() {
        let x: Vec<f32> = (0..100_000).map(|i| ((i as f32) * 0.001).sin() * (i as f32 / 100_000.0)).collect();
        let p = Peaks::new(&x);
        let (lo, hi) = p.range(0, x.len());
        assert!(hi > 0.95 && lo < -0.95);
        let (lo, hi) = p.range(1000, 1100);
        let want = x[1000..1100].iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        // block-aligned: may include a few neighbouring samples
        assert!(lo <= want.0 && hi >= want.1 && hi - want.1 < 0.01);
    }
}
