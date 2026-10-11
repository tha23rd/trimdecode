use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::iter::FusedIterator;
use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};

use crate::{downmix_into, Audio, Block, Error, ErrorKind, Info, Result};

/// Decodes one file or stream, block by block or whole, on the trimmed timeline.
///
/// Iterating gives the blocks in order ([`Block`]: interleaved samples and their channel
/// count); [`decode`](Self::decode) and [`decode_mono`](Self::decode_mono) collect what is
/// left. Every way of reading gives the same samples: the blocks, joined, are the whole-file
/// decode. After an error the iterator ends.
pub struct Decoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track: u32,
    info: Info,
    /// The file's path, or "the stream", for messages.
    name: String,
    /// The latest block, interleaved.
    buf: Vec<f32>,
    /// The channel count of a block in `buf` not handed out yet (the one read on opening).
    pending: Option<usize>,
    done: bool,
}

impl Decoder {
    /// Open a file. The format is detected from the content, with the extension as a hint.
    /// The first packet is decoded, so [`info`](Self::info) knows the channel count the
    /// decoder outputs.
    pub fn open(path: impl AsRef<Path>) -> Result<Decoder> {
        let path = path.as_ref();
        let name = path.display().to_string();
        let file = File::open(path).map_err(|e| Error::caused(ErrorKind::Io, format!("opening {name}"), e))?;
        Self::start(Box::new(file), path.extension().and_then(|e| e.to_str()), name)
    }

    /// Decode from a reader (a [`Cursor`](std::io::Cursor) over bytes in memory, say), with
    /// the file extension (`"mp3"`) as an optional hint. The reader must seek: some
    /// containers (MP4) keep their index at the end.
    pub fn from_reader<R>(mut reader: R, extension: Option<&str>) -> Result<Decoder>
    where
        R: Read + Seek + Send + Sync + 'static,
    {
        let name = "the stream".to_string();
        let len = (|| {
            let here = reader.stream_position()?;
            let end = reader.seek(SeekFrom::End(0))?;
            reader.seek(SeekFrom::Start(here))?;
            Ok::<_, std::io::Error>(end)
        })()
        .map_err(|e| Error::caused(ErrorKind::Io, format!("reading {name}"), e))?;
        Self::start(Box::new(Source { inner: reader, len }), extension, name)
    }

    fn start(source: Box<dyn MediaSource>, extension: Option<&str>, name: String) -> Result<Decoder> {
        let stream = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = extension {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(&hint, stream, Default::default(), Default::default())
            .map_err(|e| Error::caused(ErrorKind::Unsupported, format!("unrecognised audio in {name}"), e))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| Error::new(ErrorKind::NoAudio, format!("no audio track in {name}")))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| Error::new(ErrorKind::Unsupported, format!("no codec parameters in {name}")))?
            .clone();
        let (id, frames, delay, padding) = (track.id, track.num_frames, track.delay, track.padding);
        let rate = params
            .sample_rate
            .ok_or_else(|| Error::new(ErrorKind::Unsupported, format!("no sample rate in {name}")))?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| Error::caused(ErrorKind::Unsupported, format!("no decoder for {name}"), e))?;
        let info = Info {
            rate,
            channels: params.channels.as_ref().map_or(0, |c| c.count()),
            frames,
            codec: decoder.codec_info().short_name.to_string(),
            delay,
            padding,
        };
        let mut d = Decoder { format, decoder, track: id, info, name, buf: Vec::new(), pending: None, done: false };
        // The decoder's own output says how many channels there are (the container may not).
        if let Some(channels) = d.read()? {
            d.info.channels = channels;
            d.pending = Some(channels);
        }
        Ok(d)
    }

    /// What the file holds: rate, channels, length, codec, and the delay and padding trimmed.
    pub fn info(&self) -> &Info {
        &self.info
    }

    /// Decode the rest, channels interleaved. Fails if the channel count changes partway
    /// (which interleaved samples cannot represent; [`decode_mono`](Self::decode_mono) and
    /// the blocks can).
    pub fn decode(mut self) -> Result<Audio> {
        let channels = self.info.channels;
        let mut samples = Vec::new();
        while let Some(c) = self.next_raw()? {
            if c != channels {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    format!("the channel count changes from {channels} to {c} in {}", self.name),
                ));
            }
            samples.extend_from_slice(&self.buf);
        }
        Ok(Audio { samples, rate: self.info.rate, channels })
    }

    /// Decode the rest, mixed to mono block by block as [`downmix`](crate::downmix) does.
    pub fn decode_mono(mut self) -> Result<Audio> {
        let mut samples = Vec::new();
        while let Some(c) = self.next_raw()? {
            downmix_into(&mut samples, &self.buf, c);
        }
        Ok(Audio { samples, rate: self.info.rate, channels: 1 })
    }

    /// The next block into `buf`: its channel count, or `None` at the end.
    fn next_raw(&mut self) -> Result<Option<usize>> {
        match self.pending.take() {
            Some(c) => Ok(Some(c)),
            None => self.read(),
        }
    }

    /// Decode the next non-empty block of the track into `buf`. Corrupt packets are skipped,
    /// as ffmpeg skips them.
    fn read(&mut self) -> Result<Option<usize>> {
        while !self.done {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => break,
                Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(SymphoniaError::ResetRequired) => break,
                Err(e) => {
                    self.done = true;
                    return Err(Error::caused(ErrorKind::Format, format!("reading {}", self.name), e));
                }
            };
            if packet.track_id != self.track {
                continue;
            }
            let buf = match self.decoder.decode(&packet) {
                Ok(buf) => buf,
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(e) => {
                    self.done = true;
                    return Err(Error::caused(ErrorKind::Decode, format!("decoding {}", self.name), e));
                }
            };
            let channels = buf.spec().channels().count();
            if channels == 0 || buf.frames() == 0 {
                continue;
            }
            self.buf.resize(buf.samples_interleaved(), 0.0);
            buf.copy_to_slice_interleaved(&mut self.buf);
            return Ok(Some(channels));
        }
        self.done = true;
        Ok(None)
    }
}

impl Iterator for Decoder {
    type Item = Result<Block>;

    fn next(&mut self) -> Option<Result<Block>> {
        match self.next_raw() {
            Ok(Some(channels)) => Some(Ok(Block { samples: self.buf.clone(), channels })),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

impl FusedIterator for Decoder {}

impl fmt::Debug for Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoder").field("name", &self.name).field("info", &self.info).finish_non_exhaustive()
    }
}

/// A seekable reader as Symphonia's media source, with its length measured once.
struct Source<R> {
    inner: R,
    len: u64,
}

impl<R: Read> Read for Source<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R: Seek> Seek for Source<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for Source<R> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}
