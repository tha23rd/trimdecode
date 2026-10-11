# Changelog

## 0.1.0 (unreleased)

First release: the decoder from [beatgrid](https://github.com/tha23rd/beatgrid) 0.1, on its
own. Its mono decode gives the same samples as beatgrid 0.1's, bit for bit.

- `decode` and `decode_mono` for whole files; `Decoder` (`open`, `from_reader`) for blocks
  (an iterator), `info()` and the rest of a file; `info` for the metadata alone.
- `Info`: sample rate, channels, frames, duration, codec, and the encoder delay and padding
  trimmed.
- `downmix` and `downmix_into`: stereo to mono as `ffmpeg -ac 1` mixes it.
- Errors with a kind and the file they are about.
- Tests against ffmpeg format by format, and on your own files (`TRIMDECODE_CORPUS`).
- Known gap: AAC in MP4 keeps its priming samples (Symphonia does not apply the edit list).
