use std::env;
use std::fs;
use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

static NEXT_ARTIFACT_ID: AtomicU64 = AtomicU64::new(1);

const DEBUG_DIR_ENV: &str = "CODEX_TUI_DEBUG_15152_DIR";
const CAPTURE_ANSI_ENV: &str = "CODEX_TUI_DEBUG_15152_CAPTURE_ANSI";

pub(crate) fn enabled() -> bool {
    env::var_os(DEBUG_DIR_ENV).is_some()
}

pub(crate) fn capture_ansi_enabled() -> bool {
    env::var_os(CAPTURE_ANSI_ENV).is_some()
}

pub(crate) fn write_text(category: &str, label: &str, content: &str) {
    write_bytes(category, label, content.as_bytes());
}

pub(crate) fn write_bytes(category: &str, label: &str, content: &[u8]) {
    let Some(root) = debug_root() else {
        return;
    };

    let category_dir = root.join(sanitize_path_component(category));
    if let Err(err) = fs::create_dir_all(&category_dir) {
        tracing::warn!(error = %err, path = %category_dir.display(), "failed to create debug artifact dir");
        return;
    }

    let seq = NEXT_ARTIFACT_ID.fetch_add(1, Ordering::Relaxed);
    let file_name = format!("{seq:05}-{}", sanitize_path_component(label));
    let path = category_dir.join(file_name);
    if let Err(err) = fs::write(&path, content) {
        tracing::warn!(error = %err, path = %path.display(), "failed to write debug artifact");
    }
}

pub(crate) struct TeeWriter<'a, W: Write> {
    inner: &'a mut W,
    capture: Option<Vec<u8>>,
}

impl<'a, W: Write> TeeWriter<'a, W> {
    pub(crate) fn new(inner: &'a mut W, capture: bool) -> Self {
        Self {
            inner,
            capture: capture.then(Vec::new),
        }
    }

    pub(crate) fn finish_capture(self) -> Option<Vec<u8>> {
        self.capture
    }
}

impl<W: Write> Write for TeeWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        if let Some(capture) = self.capture.as_mut() {
            capture.extend_from_slice(&buf[..written]);
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.inner.write_all(buf)?;
        if let Some(capture) = self.capture.as_mut() {
            capture.extend_from_slice(buf);
        }
        Ok(())
    }
}

fn debug_root() -> Option<PathBuf> {
    env::var_os(DEBUG_DIR_ENV).map(PathBuf::from)
}

fn sanitize_path_component(input: &str) -> String {
    let mut sanitized = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }

    if sanitized.is_empty() {
        "artifact".to_string()
    } else {
        sanitized
    }
}
