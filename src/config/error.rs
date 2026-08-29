use std::fmt;
use std::io;

#[derive(Debug)]
pub enum StoreError {
    Locked,
    Corrupt,
    Io(io::Error),
    Schema {
        file: String,
        path: String,
        expected: String,
        found: String,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locked => {
                formatter.write_str("configuration is locked by another rdp-tui process")
            }
            Self::Corrupt => {
                formatter.write_str("configuration is corrupt and was preserved unchanged")
            }
            Self::Io(error) => error.fmt(formatter),
            Self::Schema {
                file,
                path,
                expected,
                found,
            } => write!(
                formatter,
                "{file}:{path}: expected {expected}, found {found}"
            ),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
