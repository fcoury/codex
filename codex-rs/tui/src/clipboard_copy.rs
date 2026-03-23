use std::fmt;
use std::io::stdout;

#[cfg(windows)]
use std::io;

use base64::Engine;
use crossterm::Command;
use ratatui::crossterm::execute;

const OSC52_CHUNK_SIZE: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CopyMethod {
    Native,
    Osc52,
}

#[derive(Debug)]
pub(crate) enum CopyError {
    ClipboardUnavailable(String),
    Osc52Failed(String),
}

impl fmt::Display for CopyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CopyError::ClipboardUnavailable(msg) => {
                write!(f, "clipboard unavailable: {msg}")
            }
            CopyError::Osc52Failed(msg) => write!(f, "osc52 copy failed: {msg}"),
        }
    }
}

impl std::error::Error for CopyError {}

pub(crate) fn copy_to_clipboard(text: &str) -> Result<CopyMethod, CopyError> {
    match try_native(text) {
        Ok(()) => Ok(CopyMethod::Native),
        Err(err) => {
            tracing::debug!("native clipboard failed, falling back to osc52: {err}");
            try_osc52(text)
        }
    }
}

fn try_native(text: &str) -> Result<(), CopyError> {
    let mut cb = arboard::Clipboard::new()
        .map_err(|err| CopyError::ClipboardUnavailable(err.to_string()))?;
    cb.set_text(text.to_string())
        .map_err(|err| CopyError::ClipboardUnavailable(err.to_string()))
}

fn try_osc52(text: &str) -> Result<CopyMethod, CopyError> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    for chunk in encoded.as_bytes().chunks(OSC52_CHUNK_SIZE) {
        let chunk =
            std::str::from_utf8(chunk).map_err(|err| CopyError::Osc52Failed(err.to_string()))?;
        execute!(stdout(), Osc52Copy(chunk.to_string()))
            .map_err(|err| CopyError::Osc52Failed(err.to_string()))?;
    }
    Ok(CopyMethod::Osc52)
}

#[derive(Debug, Clone)]
struct Osc52Copy(pub String);

impl Command for Osc52Copy {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b]52;c;{}\x07", self.0)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(std::io::Error::other(
            "tried to execute Osc52Copy using WinAPI; use ANSI instead",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}
