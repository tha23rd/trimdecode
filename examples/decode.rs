//! Decode a file to raw f32le on stdout, or describe it, for comparing with ffmpeg:
//!
//!   cargo run --release --example decode -- FILE > ours.f32
//!   ffmpeg -v error -i FILE -map 0:a:0 -f f32le - > ffmpeg.f32
//!   cargo run --release --example decode -- --mono FILE     (ffmpeg: add -ac 1)
//!   cargo run --release --example decode -- --info FILE

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let Some(file) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("usage: decode [--mono | --info] FILE");
        return ExitCode::from(2);
    };
    match run(file, flag("--mono"), flag("--info")) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(file: &str, mono: bool, info: bool) -> Result<(), Box<dyn std::error::Error>> {
    let decoder = trimdecode::Decoder::open(file)?;
    if info {
        let i = decoder.info().clone();
        let audio = decoder.decode()?;
        println!(
            "codec {} rate {} channels {} frames {:?} (decoded {}) delay {:?} padding {:?}",
            i.codec,
            i.rate,
            i.channels,
            i.frames,
            audio.frames(),
            i.delay,
            i.padding
        );
        return Ok(());
    }
    let audio = if mono { decoder.decode_mono()? } else { decoder.decode()? };
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for s in &audio.samples {
        out.write_all(&s.to_le_bytes())?;
    }
    out.flush()?;
    Ok(())
}
