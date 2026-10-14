use std::{fmt, io};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The archive is damaged or does not follow its format.
    Corrupt(String),
    /// A format feature or compression method that Packrat does not implement.
    Unsupported(String),
    /// CRC or size check of an entry failed.
    Integrity(String),
    /// An entry was refused by a safety rule.
    Blocked(String),
    Cancelled,
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O: {e}"),
            Error::Corrupt(m) => write!(f, "corrupt archive: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported: {m}"),
            Error::Integrity(m) => write!(f, "integrity check failed: {m}"),
            Error::Blocked(m) => write!(f, "blocked: {m}"),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        // Errors that travel through `Read`/`Write` wrappers keep their meaning.
        if e.kind() == io::ErrorKind::Other
            && let Some(inner) = e.get_ref()
        {
            if inner.downcast_ref::<Cancelled>().is_some() {
                return Error::Cancelled;
            }
            if let Some(Error::Integrity(m)) = inner.downcast_ref::<Error>() {
                return Error::Integrity(m.clone());
            }
        }
        Error::Io(e)
    }
}

/// Marker error carried inside `io::Error` when a long operation is cancelled from inside an `io` adapter.
#[derive(Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

impl From<Error> for io::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Io(e) => e,
            Error::Cancelled => io::Error::other(Cancelled),
            other => io::Error::other(other),
        }
    }
}

pub(crate) fn corrupt<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Corrupt(msg.into()))
}
