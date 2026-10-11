//! The timeline against ffmpeg's on your own files:
//!
//!   TRIMDECODE_CORPUS=/music/a:/music/b.flac cargo test --release --test corpus -- --nocapture
//!
//! Every file (directories are walked) is decoded by trimdecode and by ffmpeg, and must give
//! the same number of frames with no offset: the same samples for lossless codecs, and within
//! what two decoders of one lossy codec differ by. Files neither can decode are counted, not
//! failed. Nothing is written next to the files. Without TRIMDECODE_CORPUS the test passes.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::{compare, ffmpeg_decode, have_ffmpeg};

/// Two decoders of one lossy codec differ by rounding; a timeline off by a sample differs by far
/// more on music (see the summary's largest differences).
const LOSSY: f32 = 1e-3;

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
    // By codec: [files, same timeline, largest difference]; and the failures.
    let mut summary: BTreeMap<String, (usize, usize, f32)> = BTreeMap::new();
    let mut undecodable: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let decoded = trimdecode::Decoder::open(path).and_then(|d| {
            let info = d.info().clone();
            d.decode().map(|a| (info, a))
        });
        let (info, ours) = match decoded {
            Ok(x) => x,
            Err(_) => {
                *undecodable.entry(ext).or_default() += 1;
                continue;
            }
        };
        let theirs = ffmpeg_decode(path, false);
        let lossless = matches!(info.codec.as_str(), "flac" | "alac") || info.codec.starts_with("pcm");
        let tolerance = if lossless { 0.0 } else { LOSSY };
        let c = compare(&ours.samples, &theirs, ours.channels, ours.rate, tolerance);
        let key = format!("{ext}/{}", info.codec);
        let row = summary.entry(key.clone()).or_default();
        row.0 += 1;
        if c.extra_frames == 0 && c.max_diff <= tolerance {
            row.1 += 1;
            row.2 = row.2.max(c.max_diff);
        } else {
            // Files by number only: corpus paths stay out of logs that might be shared.
            failures.push(format!("file #{i} ({key}): {c:?}"));
        }
    }
    eprintln!("{:16} {:>6} {:>14} {:>12}", "ext/codec", "files", "same timeline", "max diff");
    for (k, (n, ok, diff)) in &summary {
        eprintln!("{k:16} {n:>6} {ok:>14} {diff:>12.2e}");
    }
    for (ext, n) in &undecodable {
        eprintln!("{ext:16} {n:>6} not decodable by trimdecode");
    }
    assert!(
        failures.is_empty(),
        "{} of {} files off ffmpeg's timeline:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
}
