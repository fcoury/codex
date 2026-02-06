use base64::Engine;
use std::io::Write;

/// Copy text to the system clipboard.
///
/// Over SSH, prefers OSC 52 so the text reaches the *local* terminal emulator's
/// clipboard rather than a remote X11/Wayland clipboard that the user cannot
/// access.  On a local session, tries `arboard` (native clipboard) first and
/// falls back to OSC 52 if that fails.
///
/// OSC 52 is supported by kitty, WezTerm, iTerm2, Ghostty, and others.
pub(crate) fn copy_to_clipboard(text: &str) -> Result<(), String> {
    if is_ssh_session() {
        // Over SSH the native clipboard writes to the remote machine which is
        // useless. Prefer OSC 52 which travels through the SSH tunnel to the
        // local terminal emulator.
        tracing::debug!("SSH session detected, using OSC 52 for clipboard copy. Copied: {text}");
        _ = osc52_copy(text).map_err(|osc_err| {
            tracing::warn!("OSC 52 clipboard copy failed: {osc_err}");
        });
    }

    tracing::debug!("Using native clipboard for clipboard copy. Copied: {text}");
    match arboard_copy(text) {
        Ok(()) => Ok(()),
        Err(native_err) => {
            tracing::warn!("native clipboard copy failed: {native_err}, falling back to OSC 52");
            osc52_copy(text).map_err(|osc_err| {
                format!("native clipboard: {native_err}; OSC 52 fallback: {osc_err}")
            })
        }
    }
}

/// Detect whether the current process is running inside an SSH session.
fn is_ssh_session() -> bool {
    std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some()
}

fn arboard_copy(text: &str) -> Result<(), String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("failed to set clipboard text: {e}"))
}

/// Write text to the clipboard via the OSC 52 terminal escape sequence.
fn osc52_copy(text: &str) -> Result<(), String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let sequence = format!("\x1b]52;c;{encoded}\x07");
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(sequence.as_bytes())
        .map_err(|e| format!("failed to write OSC 52: {e}"))?;
    stdout
        .flush()
        .map_err(|e| format!("failed to flush OSC 52: {e}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn osc52_encoding_roundtrips() {
        use base64::Engine;
        let text = "# Hello\n\n```rust\nfn main() {}\n```\n";
        let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        assert_eq!(decoded, text.as_bytes());
    }
}
