//! The timeline against ffmpeg's, format by format: fixtures are encoded with ffmpeg from a
//! synthetic signal, then decoded by trimdecode and by ffmpeg. Every format must give the same
//! number of frames with no offset; lossless formats the same samples, bit for bit, and lossy
//! ones the same within what two different decoders of one codec differ by.
//!
//! Needs ffmpeg (with libmp3lame and libvorbis) on the PATH; without it the tests say so and
//! pass, unless TRIMDECODE_REQUIRE_FFMPEG is set.

mod common;

use common::{compare, ffmpeg, ffmpeg_decode, have_ffmpeg, signal, write_wav, Scratch};
use trimdecode::{Decoder, ErrorKind};

/// The largest difference allowed between trimdecode's and ffmpeg's samples for a lossy codec:
/// two decoders of one codec round differently, but a timeline off by even one sample differs by
/// far more than this on the test signal (its clicks change by ~0.1 from one sample to the next).
const LOSSY: f32 = 2e-3;

struct Fixture {
    name: &'static str,
    /// The source: (sample rate, channels).
    source: (u32, u16),
    ext: &'static str,
    /// ffmpeg's output options.
    args: &'static [&'static str],
    lossless: bool,
    /// Known gap: priming frames trimdecode keeps at the start and ffmpeg trims (AAC in MP4,
    /// whose edit list Symphonia does not apply). Everything after them must match.
    kept: usize,
}

const fn lossless(name: &'static str, source: (u32, u16), ext: &'static str, args: &'static [&'static str]) -> Fixture {
    Fixture { name, source, ext, args, lossless: true, kept: 0 }
}

const fn lossy(name: &'static str, source: (u32, u16), ext: &'static str, args: &'static [&'static str]) -> Fixture {
    Fixture { name, source, ext, args, lossless: false, kept: 0 }
}

const FIXTURES: &[Fixture] = &[
    lossless("wav-s16", (44100, 2), "wav", &["-c:a", "pcm_s16le"]),
    lossless("wav-s24", (48000, 2), "wav", &["-c:a", "pcm_s24le"]),
    lossless("wav-f32", (44100, 2), "wav", &["-c:a", "pcm_f32le"]),
    lossless("wav-mono", (44100, 1), "wav", &["-c:a", "pcm_s16le"]),
    lossless("aiff", (44100, 2), "aiff", &["-c:a", "pcm_s16be"]),
    lossless("aiff-s24", (48000, 2), "aif", &["-c:a", "pcm_s24be"]),
    lossless("flac", (44100, 2), "flac", &["-c:a", "flac"]),
    lossless("flac-48k-mono", (48000, 1), "flac", &["-c:a", "flac"]),
    lossless("alac-m4a", (44100, 2), "m4a", &["-c:a", "alac"]),
    lossy("mp3-cbr", (44100, 2), "mp3", &["-c:a", "libmp3lame", "-b:a", "320k"]),
    lossy("mp3-vbr", (44100, 2), "mp3", &["-c:a", "libmp3lame", "-q:a", "2"]),
    lossy("mp3-48k", (48000, 2), "mp3", &["-c:a", "libmp3lame", "-b:a", "192k"]),
    lossy("mp3-mono", (44100, 1), "mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]),
    // No LAME/Xing header: nothing says the encoder delay, and ffmpeg drops only the
    // decoder's own 529 samples.
    lossy("mp3-no-header", (44100, 2), "mp3", &["-c:a", "libmp3lame", "-b:a", "256k", "-write_xing", "0"]),
    lossy("ogg-vorbis", (44100, 2), "ogg", &["-c:a", "libvorbis", "-q:a", "6"]),
    Fixture { kept: 1024, ..lossy("aac-m4a", (44100, 2), "m4a", &["-c:a", "aac", "-b:a", "256k"]) },
    Fixture { kept: 1024, ..lossy("aac-48k-mp4", (48000, 2), "mp4", &["-c:a", "aac", "-b:a", "192k"]) },
];

/// 3.3 s: not a whole number of frames of any codec, so the end is trimmed too.
const SECONDS: f64 = 3.3;

/// Encode every fixture into `dir`, returning (fixture, path).
fn encode_all(dir: &Scratch) -> Vec<(&'static Fixture, std::path::PathBuf)> {
    FIXTURES
        .iter()
        .map(|f| {
            let (rate, channels) = f.source;
            let source = dir.path(&format!("source-{rate}-{channels}.wav"));
            if !source.exists() {
                write_wav(&source, rate, channels, &signal(rate, SECONDS));
            }
            let out = dir.path(&format!("{}.{}", f.name, f.ext));
            let mut args = vec!["-i", source.to_str().unwrap()];
            args.extend_from_slice(f.args);
            args.push(out.to_str().unwrap());
            ffmpeg(&args);
            (f, out)
        })
        .collect()
}

#[test]
fn every_format_decodes_on_ffmpegs_timeline() {
    if !have_ffmpeg("every_format_decodes_on_ffmpegs_timeline") {
        return;
    }
    let dir = Scratch::new("parity");
    let mut failures = Vec::new();
    eprintln!(
        "{:16} {:7} {:>6} {:>2} {:>7} {:>6} {:>7}  {:>9} {:>9}",
        "fixture", "codec", "rate", "ch", "frames", "delay", "padding", "max diff", "mono diff"
    );
    for (f, path) in encode_all(&dir) {
        let decoder = Decoder::open(&path).unwrap();
        let info = decoder.info().clone();
        let ours = decoder.decode().unwrap();
        let theirs = ffmpeg_decode(&path, false);
        let tolerance = if f.lossless { 0.0 } else { LOSSY };
        let kept = f.kept.min(ours.frames());
        let c = compare(&ours.samples[kept * ours.channels..], &theirs, ours.channels, ours.rate, tolerance);

        let mono = trimdecode::decode_mono(&path).unwrap();
        let theirs_mono = ffmpeg_decode(&path, true);
        // ffmpeg mixes in its own order of operations: stereo to mono differs by rounding.
        let m = compare(&mono.samples[kept..], &theirs_mono, 1, mono.rate, tolerance.max(1e-6));

        eprintln!(
            "{:16} {:7} {:>6} {:>2} {:>7} {:>6} {:>7}  {:>9.2e} {:>9.2e}{}",
            f.name,
            info.codec,
            info.rate,
            info.channels,
            ours.frames(),
            info.delay.map_or("-".to_string(), |d| d.to_string()),
            info.padding.map_or("-".to_string(), |p| p.to_string()),
            c.max_diff,
            m.max_diff,
            if f.kept > 0 { format!("  {} priming frames kept (known gap)", f.kept) } else { String::new() }
        );
        assert_eq!(info.rate, f.source.0, "{}: rate", f.name);
        assert_eq!(info.channels, f.source.1 as usize, "{}: channels", f.name);
        if c.extra_frames != 0 || c.max_diff > tolerance {
            failures.push(format!("{}: {c:?} (tolerance {tolerance:e})", f.name));
        }
        if m.extra_frames != 0 || m.max_diff > tolerance.max(1e-6) {
            failures.push(format!("{} mono: {m:?}", f.name));
        }
        if f.kept > 0 {
            // The known gap, exactly: these frames and nothing else, and no delay reported.
            assert_eq!(info.delay, None, "{}: a delay is reported; is the gap fixed?", f.name);
        } else if let Some(frames) = info.frames {
            if frames != ours.frames() as u64 {
                failures.push(format!("{}: the file states {frames} frames, the decode has {}", f.name, ours.frames()));
            }
        }
    }
    assert!(failures.is_empty(), "not on ffmpeg's timeline:\n{}", failures.join("\n"));
}

#[test]
fn lossless_formats_give_the_source_samples() {
    if !have_ffmpeg("lossless_formats_give_the_source_samples") {
        return;
    }
    let dir = Scratch::new("lossless");
    for (f, path) in encode_all(&dir).into_iter().filter(|(f, _)| f.lossless && f.name != "wav-f32") {
        let (rate, channels) = f.source;
        let want: Vec<f32> = signal(rate, SECONDS)
            .iter()
            .flat_map(|frame| frame[..channels as usize].iter().map(|&s| f32::from(s) / 32768.0).collect::<Vec<_>>())
            .collect();
        let got = trimdecode::decode(&path).unwrap();
        assert_eq!(got.samples, want, "{}", f.name);
    }
}

#[test]
fn blocks_reader_and_whole_file_agree_for_every_format() {
    if !have_ffmpeg("blocks_reader_and_whole_file_agree_for_every_format") {
        return;
    }
    let dir = Scratch::new("blocks");
    for (f, path) in encode_all(&dir) {
        let whole = trimdecode::decode(&path).unwrap();
        let mono = trimdecode::decode_mono(&path).unwrap();
        let (mut joined, mut joined_mono, mut count) = (Vec::new(), Vec::new(), 0);
        for block in Decoder::open(&path).unwrap() {
            let block = block.unwrap();
            assert!(block.frames() > 0, "{}: an empty block", f.name);
            assert_eq!(block.channels, whole.channels, "{}", f.name);
            joined.extend_from_slice(&block.samples);
            joined_mono.extend(block.to_mono());
            count += 1;
        }
        assert!(count > 1, "{}: one block", f.name);
        assert_eq!(joined, whole.samples, "{}: the blocks are not the whole file", f.name);
        assert_eq!(joined_mono, mono.samples, "{}: the mono blocks are not the mono file", f.name);
        assert_eq!(whole.to_mono(), mono, "{}", f.name);

        let bytes = std::fs::read(&path).unwrap();
        let from_reader = Decoder::from_reader(std::io::Cursor::new(bytes), Some(f.ext)).unwrap();
        assert_eq!(from_reader.info(), Decoder::open(&path).unwrap().info(), "{}", f.name);
        assert_eq!(from_reader.decode().unwrap(), whole, "{}: from a reader", f.name);
        let bytes = std::fs::read(&path).unwrap();
        let unhinted = Decoder::from_reader(std::io::Cursor::new(bytes), None).unwrap().decode().unwrap();
        assert_eq!(unhinted, whole, "{}: from a reader without a hint", f.name);
    }
}

#[test]
fn opus_is_reported_as_unsupported() {
    if !have_ffmpeg("opus_is_reported_as_unsupported") {
        return;
    }
    let dir = Scratch::new("opus");
    let source = dir.path("source.wav");
    write_wav(&source, 48000, 2, &signal(48000, 1.0));
    let out = dir.path("opus.ogg");
    let probe = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&source)
        .args(["-c:a", "libopus"])
        .arg(&out)
        .status()
        .unwrap();
    if !probe.success() {
        eprintln!("opus_is_reported_as_unsupported: skipped, ffmpeg has no libopus");
        return;
    }
    let e = trimdecode::decode(&out).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unsupported, "{e:#}");
}
