use std::fmt;

/// What went wrong, broadly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The file could not be opened or read.
    Io,
    /// Not audio trimdecode recognises, or a codec it has no decoder for.
    Unsupported,
    /// The file has no audio track.
    NoAudio,
    /// The container could not be read past some point.
    Format,
    /// The decoder failed (a corrupt packet is skipped instead, as ffmpeg skips it).
    Decode,
}

/// A decoding error: what was being done, to which file, and why.
///
/// `Display` gives what was being done (`decoding track.mp3`); the alternate form (`{:#}`)
/// adds the causes (`decoding track.mp3: malformed stream: ...`), as `anyhow` prints a chain.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    context: String,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, context: String) -> Self {
        Error { kind, context, source: None }
    }

    pub(crate) fn caused(
        kind: ErrorKind,
        context: String,
        source: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    ) -> Self {
        Error { kind, context, source: Some(source.into()) }
    }

    /// What went wrong, broadly.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.context)?;
        if f.alternate() {
            let mut cause = std::error::Error::source(self);
            while let Some(e) = cause {
                write!(f, ": {e}")?;
                cause = e.source();
            }
        }
        Ok(())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|e| e as &(dyn std::error::Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_alternate_form_adds_the_causes() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let e = Error::caused(ErrorKind::Io, "opening a.mp3".into(), io);
        assert_eq!(e.to_string(), "opening a.mp3");
        assert_eq!(format!("{e:#}"), "opening a.mp3: no such file");
        assert_eq!(e.kind(), ErrorKind::Io);
        assert_eq!(
            format!("{:#}", Error::new(ErrorKind::NoAudio, "no audio track in a.mp4".into())),
            "no audio track in a.mp4"
        );
    }
}
