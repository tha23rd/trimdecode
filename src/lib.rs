//! Decode audio files to `f32` samples on the timeline ffmpeg (and DJ software) uses: encoder
//! delay and padding trimmed, so a time in seconds means the same sample everywhere.
//!
//! ```no_run
//! // The whole file, channels interleaved, at its own sample rate.
//! let audio = trimdecode::decode("track.mp3")?;
//! println!("{} Hz, {} channels, {:.3} s", audio.rate, audio.channels, audio.duration());
//!
//! // Mono, mixed the way `ffmpeg -ac 1` mixes stereo: (L + R) / sqrt 2.
//! let mono = trimdecode::decode_mono("track.flac")?;
//! assert_eq!(mono.channels, 1);
//!
//! // Block by block, without holding the track in memory.
//! let mut decoder = trimdecode::Decoder::open("track.m4a")?;
//! println!("{} at {} Hz, {:?} frames", decoder.info().codec, decoder.info().rate, decoder.info().frames);
//! for block in &mut decoder {
//!     let block = block?;
//!     // block.samples: interleaved, block.channels per frame
//! }
//! # Ok::<(), trimdecode::Error>(())
//! ```
//!
//! # The timeline
//!
//! Lossy encoders add audio that is not part of the recording: an *encoder delay* of silence
//! or priming samples before it (1105 samples for a typical LAME MP3, 1024 or 2112 for AAC)
//! and *padding* after it, to fill the last frame. A file that records the amounts (a LAME or
//! Xing header in an MP3, an edit list or iTunes gapless data in an MP4, granule positions in
//! Ogg) lets a decoder drop them: gapless decoding. ffmpeg does, and so does trimdecode, so
//! sample `n` here is sample `n` of `ffmpeg -i FILE -f f32le -`, the same number of samples,
//! with no offset. An MP3 without such a header gets what ffmpeg gives it too: the decoder's
//! own 529-sample delay dropped and nothing else.
//!
//! This matters wherever a time is stored apart from the audio: beat grids, cue points and
//! loops in DJ software, labels, subtitles, alignments. DJ software stores them on this
//! trimmed timeline; a decoder that keeps the delay puts every one of them 25-50 ms late.
//! The tests check the timeline against ffmpeg, sample for sample, format by format.
//!
//! Known gaps (see the README): AAC in an MP4 whose edit list says to skip the priming keeps
//! it (Symphonia does not apply edit lists yet), and some MP3s without a LAME/Xing header end
//! one frame (1152 samples) before ffmpeg's decode of them; their starts line up.
//!
//! # Formats
//!
//! Everything [Symphonia](https://github.com/pdeljanov/Symphonia) decodes, through it: MP3,
//! FLAC, WAV, AIFF, Ogg Vorbis, AAC and ALAC in MP4/M4A, CAF and Matroska/WebM (not Opus).
//! Samples come out as `f32`, nominally in [-1, 1], at the file's own sample rate:
//! trimdecode never resamples.
//!
//! # Mono
//!
//! [`downmix`] (and [`decode_mono`], [`Audio::to_mono`]) mixes stereo to mono as
//! (L + R) / sqrt 2, which is what `ffmpeg -ac 1` does, rather than the average; more than
//! two channels are averaged.

mod decoder;
mod error;

use std::path::Path;

pub use decoder::Decoder;
pub use error::{Error, ErrorKind};

/// A `Result` with trimdecode's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Decoded audio: `f32` samples, channels interleaved, at the file's own sample rate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Audio {
    /// The samples, frame by frame: `channels` values per frame (one for mono).
    pub samples: Vec<f32>,
    /// Sample rate (Hz), the file's own.
    pub rate: u32,
    /// Channels per frame.
    pub channels: usize,
}

impl Audio {
    /// The number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        self.samples.len().checked_div(self.channels).unwrap_or(0)
    }

    /// Length in seconds.
    pub fn duration(&self) -> f64 {
        if self.rate == 0 {
            return 0.0;
        }
        self.frames() as f64 / self.rate as f64
    }

    /// The audio mixed to mono as [`downmix`] does.
    pub fn to_mono(&self) -> Audio {
        Audio { samples: downmix(&self.samples, self.channels), rate: self.rate, channels: 1.min(self.channels) }
    }

    /// The audio mixed to mono as [`downmix`] does, reusing the buffer when it is mono already.
    pub fn into_mono(self) -> Audio {
        if self.channels <= 1 {
            return self;
        }
        self.to_mono()
    }
}

/// What a file holds, known once it is opened ([`Decoder::info`], [`info`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Info {
    /// Sample rate (Hz), the file's own.
    pub rate: u32,
    /// Channels per frame, as the decoder outputs them.
    pub channels: usize,
    /// Frames on the trimmed timeline, as the file states them; `None` when it does not. The
    /// decode itself is authoritative: count what it returns when you need the exact length.
    pub frames: Option<u64>,
    /// The codec's short name: `mp3`, `flac`, `aac`, `alac`, `vorbis`, `pcm_s16le`, ...
    pub codec: String,
    /// Encoder delay (frames) the file declares and the decode drops from the start; `None`
    /// when it declares none.
    pub delay: Option<u32>,
    /// Padding (frames) the file declares and the decode drops from the end; `None` when it
    /// declares none.
    pub padding: Option<u32>,
}

impl Info {
    /// Length in seconds, from [`frames`](Self::frames).
    pub fn duration(&self) -> Option<f64> {
        let frames = self.frames?;
        (self.rate > 0).then(|| frames as f64 / self.rate as f64)
    }
}

/// One decoded block (a packet's worth of audio), from [`Decoder`]'s iterator.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    /// The samples, channels interleaved; never empty.
    pub samples: Vec<f32>,
    /// Channels per frame.
    pub channels: usize,
}

impl Block {
    /// The number of frames in the block.
    pub fn frames(&self) -> usize {
        self.samples.len().checked_div(self.channels).unwrap_or(0)
    }

    /// The block mixed to mono as [`downmix`] does.
    pub fn to_mono(&self) -> Vec<f32> {
        downmix(&self.samples, self.channels)
    }
}

/// Decode a whole file: channels interleaved, at its own sample rate, on the trimmed timeline.
///
/// The format is detected from the content, with the extension as a hint.
pub fn decode(path: impl AsRef<Path>) -> Result<Audio> {
    Decoder::open(path)?.decode()
}

/// Decode a whole file to mono ([`downmix`]), at its own sample rate, on the trimmed timeline.
pub fn decode_mono(path: impl AsRef<Path>) -> Result<Audio> {
    Decoder::open(path)?.decode_mono()
}

/// What a file holds (rate, channels, length, codec), decoding only its first packet.
pub fn info(path: impl AsRef<Path>) -> Result<Info> {
    Ok(Decoder::open(path)?.info().clone())
}

/// Mix interleaved frames of `channels` channels to mono the way `ffmpeg -ac 1` mixes stereo:
/// (L + R) / sqrt 2, not the average. Mono is copied; more than two channels are averaged.
/// A trailing partial frame is dropped.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(interleaved.len().checked_div(channels).unwrap_or(0));
    downmix_into(&mut out, interleaved, channels);
    out
}

/// [`downmix`], appending to `out` instead of allocating.
pub fn downmix_into(out: &mut Vec<f32>, interleaved: &[f32], channels: usize) {
    let scale = match channels {
        0 => return,
        1 => return out.extend_from_slice(interleaved),
        2 => std::f32::consts::FRAC_1_SQRT_2,
        n => 1.0 / n as f32,
    };
    out.extend(interleaved.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() * scale));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_mixes_as_ffmpeg_does_not_as_the_average() {
        let mono = downmix(&[0.5, 0.25, -1.0, 1.0, 0.1, 0.0], 2);
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert_eq!(mono, vec![(0.5 + 0.25) * s, 0.0, 0.1 * s]);
        assert_eq!(downmix(&[0.3, 0.6, 0.9], 3), vec![(0.3f32 + 0.6 + 0.9) * (1.0 / 3.0)]);
        assert_eq!(downmix(&[0.3, 0.6], 1), vec![0.3, 0.6]);
        assert!(downmix(&[0.3, 0.6], 0).is_empty());
        assert_eq!(downmix(&[0.5, 0.5, 0.25], 2).len(), 1, "a partial frame is dropped");
    }

    #[test]
    fn audio_reports_frames_duration_and_mono() {
        let a = Audio { samples: vec![0.5; 8], rate: 4, channels: 2 };
        assert_eq!(a.frames(), 4);
        assert_eq!(a.duration(), 1.0);
        let m = a.to_mono();
        assert_eq!((m.channels, m.frames(), m.rate), (1, 4, 4));
        assert_eq!(a.clone().into_mono(), m);
        assert_eq!(m.clone().into_mono(), m);
        assert_eq!(Audio::default().frames(), 0);
        assert_eq!(Audio::default().duration(), 0.0);
    }

    #[test]
    fn decoders_can_move_between_threads() {
        fn send<T: Send>() {}
        send::<Decoder>();
        send::<Error>();
        fn sync<T: Sync>() {}
        sync::<Error>();
    }
}
