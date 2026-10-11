//! Blocks, whole files, readers and errors, on WAV files written here (no ffmpeg needed).

mod common;

use common::{signal, write_wav, Scratch};
use trimdecode::{downmix, Decoder, ErrorKind};

#[test]
fn blocks_keep_the_channels_and_join_into_the_whole_file() {
    let dir = Scratch::new("blocks-wav");
    let path = dir.path("stereo.wav");
    let frames = signal(48000, 2.5);
    write_wav(&path, 48000, 2, &frames);

    let mut decoder = Decoder::open(&path).unwrap();
    let info = decoder.info().clone();
    assert_eq!((info.rate, info.channels, info.frames), (48000, 2, Some(frames.len() as u64)));
    assert_eq!(info.codec, "pcm_s16le");
    assert_eq!(info.duration(), Some(2.5));
    let mut joined = Vec::new();
    let mut joined_mono = Vec::new();
    let mut blocks = 0;
    for block in &mut decoder {
        let block = block.unwrap();
        assert_eq!(block.channels, 2);
        assert!(block.frames() > 0);
        joined.extend_from_slice(&block.samples);
        joined_mono.extend(block.to_mono());
        blocks += 1;
    }
    assert!(blocks > 1, "one block");
    assert!(decoder.next().is_none(), "the iterator stays ended");

    // The samples are the file's, exactly: 16-bit PCM over 32768, channels interleaved.
    let want: Vec<f32> = frames.iter().flat_map(|f| [f32::from(f[0]) / 32768.0, f32::from(f[1]) / 32768.0]).collect();
    assert_eq!(joined, want);
    let whole = trimdecode::decode(&path).unwrap();
    assert_eq!((whole.rate, whole.channels, whole.frames()), (48000, 2, frames.len()));
    assert_eq!(whole.samples, joined);
    let mono = trimdecode::decode_mono(&path).unwrap();
    assert_eq!(mono.channels, 1);
    assert_eq!(mono.samples, downmix(&joined, 2));
    assert_eq!(mono.samples, joined_mono);
    assert_eq!(whole.to_mono(), mono);
    assert_eq!(trimdecode::info(&path).unwrap(), info);
}

#[test]
fn reading_part_of_a_file_then_the_rest_gives_the_whole_file() {
    let dir = Scratch::new("blocks-rest");
    let path = dir.path("stereo.wav");
    write_wav(&path, 44100, 2, &signal(44100, 2.0));
    let whole = trimdecode::decode(&path).unwrap();
    let mut decoder = Decoder::open(&path).unwrap();
    let first = decoder.next().unwrap().unwrap();
    let rest = decoder.decode().unwrap();
    assert_eq!([first.samples, rest.samples].concat(), whole.samples);
}

#[test]
fn mono_files_pass_through() {
    let dir = Scratch::new("blocks-mono");
    let path = dir.path("mono.wav");
    let frames = signal(22050, 1.0);
    write_wav(&path, 22050, 1, &frames);
    let whole = trimdecode::decode(&path).unwrap();
    let mono = trimdecode::decode_mono(&path).unwrap();
    assert_eq!(whole.channels, 1);
    assert_eq!(whole, mono);
    assert_eq!(whole.samples, frames.iter().map(|f| f32::from(f[0]) / 32768.0).collect::<Vec<_>>());
}

#[test]
fn a_reader_decodes_as_the_file_does() {
    let dir = Scratch::new("blocks-reader");
    let path = dir.path("stereo.wav");
    write_wav(&path, 44100, 2, &signal(44100, 1.5));
    let bytes = std::fs::read(&path).unwrap();
    for hint in [Some("wav"), None] {
        let d = Decoder::from_reader(std::io::Cursor::new(bytes.clone()), hint).unwrap();
        assert_eq!(d.info(), Decoder::open(&path).unwrap().info());
        assert_eq!(d.decode().unwrap(), trimdecode::decode(&path).unwrap());
    }
}

#[test]
fn errors_say_what_and_which_file() {
    let dir = Scratch::new("blocks-errors");
    let missing = dir.path("missing.mp3");
    let e = trimdecode::decode(&missing).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Io);
    assert_eq!(e.to_string(), format!("opening {}", missing.display()));
    assert!(format!("{e:#}").len() > e.to_string().len(), "{e:#}: no cause");

    let junk = dir.path("junk.mp3");
    std::fs::write(&junk, b"this is not audio at all, not even close").unwrap();
    let e = trimdecode::decode_mono(&junk).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unsupported);
    assert!(e.to_string().starts_with("unrecognised audio in "), "{e}");

    let e = Decoder::from_reader(std::io::Cursor::new(vec![0u8; 64]), None).unwrap_err();
    assert_eq!(e.kind(), ErrorKind::Unsupported);
    assert_eq!(e.to_string(), "unrecognised audio in the stream");
}
