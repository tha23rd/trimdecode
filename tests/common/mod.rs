//! Test helpers: a synthetic stereo signal, a WAV writer, and ffmpeg (when installed) to
//! encode fixtures and decode them for comparison.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A scratch directory for one test, removed when dropped.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("trimdecode-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A deterministic stereo test signal: a click every 250 ms on the left, a rising tone and
/// noise bursts on the right, so a shift of one sample, or the channels swapped, shows.
pub fn signal(rate: u32, seconds: f64) -> Vec<[i16; 2]> {
    let n = (seconds * rate as f64) as usize;
    let mut noise = 0x2545_f491u32;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate as f64;
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            let white = noise as f64 / u32::MAX as f64 - 0.5;
            let since = t % 0.25;
            let click = 0.7 * (-since * 60.0).exp() * (2.0 * std::f64::consts::PI * 1000.0 * since).sin();
            let tone = 0.3 * (2.0 * std::f64::consts::PI * (200.0 + 400.0 * t) * t).sin();
            let burst = if (t * 3.0) % 1.0 < 0.1 { 0.4 * white } else { 0.0 };
            let q = |x: f64| (x.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            [q(click + 0.05 * white), q(tone + burst)]
        })
        .collect()
}

/// A 16-bit PCM WAV file of `channels`-channel frames (the first `channels` of each pair).
pub fn write_wav(path: &Path, rate: u32, channels: u16, frames: &[[i16; 2]]) {
    let block = 2 * channels as u32;
    let data = frames.len() as u32 * block;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * block).to_le_bytes());
    b.extend_from_slice(&(block as u16).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for f in frames {
        for s in &f[..channels as usize] {
            b.extend_from_slice(&s.to_le_bytes());
        }
    }
    std::fs::write(path, b).unwrap();
}

/// Whether ffmpeg is on the PATH. Tests that need it say so and pass without it, unless
/// `TRIMDECODE_REQUIRE_FFMPEG` is set (CI sets it where ffmpeg is installed).
pub fn have_ffmpeg(test: &str) -> bool {
    let ok = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        assert!(std::env::var_os("TRIMDECODE_REQUIRE_FFMPEG").is_none(), "{test}: ffmpeg is required but not found");
        eprintln!("{test}: skipped, ffmpeg not found");
    }
    ok
}

/// Run ffmpeg quietly with `args`, panicking with its message on failure.
pub fn ffmpeg(args: &[&str]) {
    let out = Command::new("ffmpeg").args(["-v", "error", "-y"]).args(args).output().expect("running ffmpeg");
    assert!(out.status.success(), "ffmpeg {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// ffmpeg's decode of `path` as f32 (channels interleaved, or `-ac 1`) and its channel count.
pub fn ffmpeg_decode(path: &Path, mono: bool) -> Vec<f32> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-i"]).arg(path).args(["-map", "0:a:0", "-f", "f32le"]);
    if mono {
        cmd.args(["-ac", "1"]);
    }
    let out = cmd.arg("-").output().expect("running ffmpeg");
    assert!(out.status.success(), "ffmpeg decoding {}: {}", path.display(), String::from_utf8_lossy(&out.stderr));
    out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

/// How two decodes of the same file compare.
#[derive(Debug)]
pub struct Comparison {
    /// Our frames minus ffmpeg's.
    pub extra_frames: i64,
    /// The largest difference between aligned samples.
    pub max_diff: f32,
    /// How many aligned samples differ by more than the tolerance.
    pub beyond: usize,
    /// The shift (frames) that lines ours up with ffmpeg's best: positive when ours is late.
    pub lag: i64,
}

/// Compare our interleaved samples with ffmpeg's. The lag is searched only when the
/// samples differ more than `tolerance`, to say why.
pub fn compare(ours: &[f32], theirs: &[f32], channels: usize, rate: u32, tolerance: f32) -> Comparison {
    let extra_frames = (ours.len() as i64 - theirs.len() as i64) / channels as i64;
    let n = ours.len().min(theirs.len());
    let diffs = || ours[..n].iter().zip(&theirs[..n]).map(|(a, b)| (a - b).abs());
    let max_diff = diffs().fold(0.0f32, f32::max);
    let beyond = diffs().filter(|d| *d > tolerance).count();
    let lag = if max_diff <= tolerance { 0 } else { lag(ours, theirs, channels, rate) };
    Comparison { extra_frames, max_diff, beyond, lag }
}

/// The shift within ±50 ms that best lines up the two signals' first channel, over the
/// loudest half second of their first 30 s: 0 when they are on the same timeline.
pub fn lag(ours: &[f32], theirs: &[f32], channels: usize, rate: u32) -> i64 {
    let a: Vec<f32> = ours.iter().step_by(channels).copied().collect();
    let b: Vec<f32> = theirs.iter().step_by(channels).copied().collect();
    let max_lag = (rate / 20) as i64;
    let win = (rate / 2) as usize;
    let limit = b.len().min(a.len()).min(30 * rate as usize).saturating_sub(win + max_lag as usize);
    let energy = |s: usize| b[s..(s + win).min(b.len())].iter().map(|v| v * v).sum::<f32>();
    let start = (max_lag as usize..limit.max(max_lag as usize + 1))
        .step_by(win)
        .max_by(|&x, &y| energy(x).total_cmp(&energy(y)))
        .unwrap_or(0);
    let ssd = |lag: i64| {
        (0..win)
            .filter_map(|i| {
                let j = start as i64 + i as i64 + lag;
                (j >= 0 && (j as usize) < a.len() && start + i < b.len())
                    .then(|| (a[j as usize] - b[start + i]).powi(2))
            })
            .sum::<f32>()
    };
    (-max_lag..=max_lag).map(|l| (l, ssd(l))).min_by(|x, y| x.1.total_cmp(&y.1)).map_or(0, |(l, _)| l)
}
