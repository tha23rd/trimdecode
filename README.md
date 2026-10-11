# trimdecode

Decode audio files to `f32` samples on the same timeline as ffmpeg: encoder delay and
padding trimmed, so a time in seconds points at the same sample in your code, in ffmpeg,
and in the DJ software that made the cue points.

```rust
// The whole file: channels interleaved, at the file's own sample rate.
let audio = trimdecode::decode("track.mp3")?;
println!("{} Hz, {} channels, {:.3} s", audio.rate, audio.channels, audio.duration());

// Mono, mixed as `ffmpeg -ac 1` mixes stereo: (L + R) / √2.
let mono = trimdecode::decode_mono("track.flac")?;

// Block by block, without the whole track in memory.
let mut decoder = trimdecode::Decoder::open("track.m4a")?;
let info = decoder.info(); // rate, channels, frames, codec, delay and padding trimmed
for block in &mut decoder {
    let block = block?; // block.samples (interleaved), block.channels
}
```

It is a thin layer over [Symphonia](https://github.com/pdeljanov/Symphonia), pure Rust,
with no system libraries: MP3, FLAC, WAV, AIFF, Ogg Vorbis, ALAC and AAC in MP4/M4A, CAF and
Matroska. It never resamples.

## Why the timeline matters

Lossy encoders add audio that is not in the recording. An MP3 starts with an *encoder delay*
(1105 samples from LAME, counting the decoder's own 529) and ends with *padding* to fill its
last frame; AAC starts with 1024 or 2112 priming samples. Most files record how much (a LAME
or Xing header in an MP3, an edit list in an MP4, granule positions in Ogg), and a gapless
decoder drops it. ffmpeg does. A decoder that does not hands you the same music 25-50 ms
later.

Nobody hears 25 ms of silence, so it rarely matters for playback. It matters a lot when a
time is stored apart from the audio. DJ software is the clearest case: Engine DJ, rekordbox,
Serato and Traktor store beat grids, cue points and loops as times into the decoded track.
Engine DJ's grids sit on ffmpeg's trimmed timeline: adding an MP3's encoder delay back moves
every grid by exactly that much. So a beat grid computed from audio that kept the delay is
25 ms late on the player, enough to flam against every kick, and a cue read from the
library and applied to that audio lands early. The same goes for stems, waveforms and
anything else that has to line up across tools: one timeline, to the sample.

trimdecode exists so that every analysis built on it (beat grids, keys, cue points, track
fingerprints and alignments) lands on that one timeline, and it is tested against ffmpeg,
sample for sample.

## What you get

| | |
|---|---|
| `decode(path)` | the whole file, channels interleaved: [`Audio`] `{ samples, rate, channels }` |
| `decode_mono(path)` | the whole file mixed to mono ([`downmix`]) |
| `Decoder::open(path)`, `Decoder::from_reader(reader, ext)` | a decoder: `info()`, an iterator of blocks, `decode()`, `decode_mono()` |
| `info(path)` | [`Info`]: `rate`, `channels`, `frames`, `duration()`, `codec`, `delay`, `padding` |
| `downmix(interleaved, channels)` | stereo to mono as (L + R) / √2 |

Samples are `f32`, nominally in [-1, 1], at the file's own sample rate. The blocks are the
decoder's packets in order; joined, they are exactly what `decode` returns, and the mono
blocks exactly what `decode_mono` returns. `from_reader` takes anything that reads and seeks
(a `Cursor` over bytes in memory), with the file extension as an optional hint.

**Mono** is (L + R) / √2 for stereo, which is what `ffmpeg -ac 1` computes (to within
float rounding: at most 6e-8, as ffmpeg multiplies before adding), not the average. More
than two channels are averaged.

**Errors** carry what was being done and to which file; `{:#}` adds the cause
(`decoding track.mp3: malformed stream: ...`), and `kind()` says what broadly went wrong. A
corrupt packet is skipped, as ffmpeg skips it.

## The timeline, format by format

Checked against ffmpeg 8 by `tests/ffmpeg_parity.rs` (fixtures encoded by ffmpeg from a
synthetic signal): the same number of frames, no offset, and the samples below.

| format | trimmed | against ffmpeg |
|---|---|---|
| MP3 with a LAME or Xing header | the declared delay (plus the decoder's 529) and padding | same frames; samples within 2e-6 |
| MP3 without one | the decoder's 529 samples, as ffmpeg (nothing says more) | same frames; samples within 2e-6 |
| Ogg Vorbis | as the granule positions say | same frames; samples within 3e-7 |
| FLAC, WAV, AIFF, ALAC in MP4 | nothing to trim | identical, bit for bit |
| AAC in MP4/M4A | **not yet: see below** | the priming samples are kept |

### Known gap: AAC in MP4

Symphonia 0.6 reads an MP4's edit list but does not apply it, so the AAC encoder's priming
samples stay at the start: 1024 for ffmpeg's and most encoders, 2112 for Apple's. Decoded
AAC is that much (23-48 ms) late against ffmpeg's timeline; the rest of the audio is the same
within 2e-3 (Symphonia's and ffmpeg's AAC decoders round differently). `Info::delay` is
`None` for these files. The parity test pins this, so a fix shows up as a test to update.

It is left so in 0.1 on purpose: the projects trimdecode was extracted from stored results
on exactly this decode, and changing it changes their AAC results. Trimming the edit list's
media time is the planned fix. Until then, if AAC timing matters, decode AAC with ffmpeg or
shift by the priming yourself.

## Testing

```sh
cargo test --release                       # unit tests, and the ffmpeg parity tests if ffmpeg is installed
TRIMDECODE_CORPUS=~/Music cargo test --release --test corpus -- --nocapture   # your own files
```

The parity tests need `ffmpeg` with libmp3lame and libvorbis on the `PATH`; without it they
say so and pass (CI sets `TRIMDECODE_REQUIRE_FFMPEG` where it installs ffmpeg, so they cannot
be skipped there). The corpus test reads the files it is given and writes nothing; it reports
files by number, not by name. `cargo run --release --example decode -- [--mono|--info] FILE`
writes raw f32le to stdout, for comparing with
`ffmpeg -i FILE -map 0:a:0 -f f32le -` by hand.

## License

MIT or Apache-2.0, at your option. Decoding is done by Symphonia (MPL-2.0).

[`Audio`]: https://docs.rs/trimdecode/latest/trimdecode/struct.Audio.html
[`Info`]: https://docs.rs/trimdecode/latest/trimdecode/struct.Info.html
[`downmix`]: https://docs.rs/trimdecode/latest/trimdecode/fn.downmix.html
