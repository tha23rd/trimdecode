//! The timeline against ffmpeg's on your own files:
//!
//!   TRIMDECODE_CORPUS=/music/a:/music/b.flac cargo test --release --test corpus -- --nocapture
//!
//! Every file (directories are walked) is decoded by trimdecode and by ffmpeg and must start on
//! the same sample (no offset), and then give the same samples: identical for lossless codecs,
//! within what two decoders of one lossy codec differ by. Allowed and counted apart, because
//! they leave every time where it was:
//!
//! - an MP3 that ends one frame (1152 samples) before ffmpeg's decode of it (seen on MP3s
//!   without a LAME/Xing header, and damaged ones);
//! - an MP3 that differs from ffmpeg by more than rounding in a few of its frames (at most 1%,
//!   and by at most 1e-2): loud, clipping passages that two MP3 decoders round differently;
//! - AAC in MP4 with its priming samples kept (the known gap: an edit list ffmpeg applies).
//!
//! Files neither can decode are counted, not failed. Nothing is written next to the files, and
//! files are reported by number, not name. Without TRIMDECODE_CORPUS the test passes.
//! TRIMDECODE_CORPUS_JOBS sets the parallel workers (default: half the CPUs).

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use common::{compare, ffmpeg_decode, have_ffmpeg, lag};

/// Two decoders of one lossy codec differ by rounding; a timeline off by a sample differs by
/// far more on music.
const LOSSY: f32 = 1e-3;

/// What a file's comparison with ffmpeg came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Same,
    OneMp3FrameShort,
    LocalDifference,
    AacPrimingKept,
    Undecodable,
    Off,
}

fn files(root: &Path, out: &mut Vec<PathBuf>) {
    if root.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(root).unwrap().map(|e| e.unwrap().path()).collect();
        entries.sort();
        for e in entries {
            files(&e, out);
        }
    } else if root.is_file() {
        out.push(root.to_path_buf());
    }
}

/// Decode one file both ways: (ext/codec, outcome, largest difference, details).
fn check(path: &Path) -> (String, Outcome, f32, String) {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let decoded = trimdecode::Decoder::open(path).and_then(|d| {
        let info = d.info().clone();
        d.decode().map(|a| (info, a))
    });
    let Ok((info, ours)) = decoded else {
        return (ext, Outcome::Undecodable, 0.0, String::new());
    };
    let key = format!("{ext}/{}", info.codec);
    let theirs = ffmpeg_decode(path, false);
    let ch = ours.channels.max(1);
    let lossless = matches!(info.codec.as_str(), "flac" | "alac") || info.codec.starts_with("pcm");
    let tolerance = if lossless { 0.0 } else { LOSSY };
    let c = compare(&ours.samples, &theirs, ch, ours.rate, tolerance);
    let offset = if c.max_diff <= tolerance { 0 } else { lag(&ours.samples, &theirs, ch, ours.rate) };
    let details = format!("{c:?}, offset {offset}");
    let outcome = if offset == 0 && c.extra_frames == 0 && c.beyond == 0 {
        Outcome::Same
    } else if offset == 0 && c.beyond == 0 && info.codec == "mp3" && c.extra_frames == -1152 {
        Outcome::OneMp3FrameShort
    } else if offset == 0 && c.extra_frames == 0 && info.codec == "mp3" && c.max_diff <= 1e-2 && {
        // MP3 frames (1152 samples) with a difference beyond rounding: at most 1% of them.
        let n = ours.samples.len().min(theirs.len());
        let mut frames: Vec<usize> =
            (0..n).filter(|&i| (ours.samples[i] - theirs[i]).abs() > tolerance).map(|i| i / ch / 1152).collect();
        frames.dedup();
        frames.len() * 100 <= ours.frames() / 1152
    } {
        Outcome::LocalDifference
    } else if info.codec == "aac" && offset > 0 && c.extra_frames == offset {
        let shifted = compare(&ours.samples[offset as usize * ch..], &theirs, ch, ours.rate, tolerance);
        if shifted.extra_frames == 0 && shifted.beyond == 0 {
            Outcome::AacPrimingKept
        } else {
            Outcome::Off
        }
    } else {
        Outcome::Off
    };
    (key, outcome, c.max_diff, details)
}

#[test]
fn the_corpus_decodes_on_ffmpegs_timeline() {
    let Some(corpus) = std::env::var_os("TRIMDECODE_CORPUS") else {
        eprintln!("the_corpus_decodes_on_ffmpegs_timeline: skipped, TRIMDECODE_CORPUS is not set");
        return;
    };
    if !have_ffmpeg("the_corpus_decodes_on_ffmpegs_timeline") {
        return;
    }
    let mut paths = Vec::new();
    for root in std::env::split_paths(&corpus) {
        files(&root, &mut paths);
    }
    let jobs = std::env::var("TRIMDECODE_CORPUS_JOBS")
        .ok()
        .and_then(|j| j.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1)));
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = paths.get(i) else { break };
                let r = check(path);
                results.lock().unwrap().push((i, r));
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| r.0);

    // By ext/codec: files per outcome, and the largest difference among the same ones.
    let mut summary: BTreeMap<String, (BTreeMap<Outcome, usize>, f32)> = BTreeMap::new();
    let mut failures = Vec::new();
    for (i, (key, outcome, diff, details)) in &results {
        let row = summary.entry(key.clone()).or_default();
        *row.0.entry(*outcome).or_default() += 1;
        if *outcome == Outcome::Same {
            row.1 = row.1.max(*diff);
        }
        if *outcome == Outcome::Off {
            // Files by number only: corpus paths stay out of logs that might be shared.
            failures.push(format!("file #{i} ({key}): {details}"));
        }
    }
    eprintln!("{} files, {jobs} workers", paths.len());
    eprintln!(
        "{:14} {:>6} {:>6} {:>9} {:>9} {:>9} {:>6} {:>12}",
        "ext/codec", "files", "same", "mp3 -1fr", "mp3 local", "aac prime", "off", "max diff"
    );
    for (k, (counts, diff)) in &summary {
        let n = |o: Outcome| counts.get(&o).copied().unwrap_or(0);
        if n(Outcome::Undecodable) > 0 {
            eprintln!("{k:14} {:>6} not decodable by trimdecode", n(Outcome::Undecodable));
            continue;
        }
        eprintln!(
            "{k:14} {:>6} {:>6} {:>9} {:>9} {:>9} {:>6} {:>12.2e}",
            counts.values().sum::<usize>(),
            n(Outcome::Same),
            n(Outcome::OneMp3FrameShort),
            n(Outcome::LocalDifference),
            n(Outcome::AacPrimingKept),
            n(Outcome::Off),
            diff
        );
    }
    assert!(
        failures.is_empty(),
        "{} of {} files off ffmpeg's timeline:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
}
