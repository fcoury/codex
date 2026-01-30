use std::path::PathBuf;

use codex_core::config::set_project_trust_level;
use codex_core::git_info::resolve_root_git_project_for_trust;
use codex_protocol::config_types::TrustLevel;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;
use std::sync::Arc;

use crate::key_hint;
use crate::key_hint::primary_binding;
use crate::keymap::TuiKeymap;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use crate::onboarding::onboarding_screen::StepStateProvider;
use crate::render::Insets;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::render::renderable::RenderableExt as _;
use crate::selection_list::selection_option_row;

use super::onboarding_screen::StepState;
pub(crate) struct TrustDirectoryWidget {
    pub codex_home: PathBuf,
    pub cwd: PathBuf,
    pub is_git_repo: bool,
    pub selection: Option<TrustDirectorySelection>,
    pub highlighted: TrustDirectorySelection,
    pub error: Option<String>,
    pub keymap: Arc<TuiKeymap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustDirectorySelection {
    Trust,
    DontTrust,
}

impl WidgetRef for &TrustDirectoryWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let mut column = ColumnRenderable::new();

        column.push(Line::from(vec![
            "> ".into(),
            "You are running Codex in ".bold(),
            self.cwd.to_string_lossy().to_string().into(),
        ]));
        column.push("");

        let guidance = if self.is_git_repo {
            "Since this folder is version controlled, you may wish to allow Codex to work in this folder without asking for approval."
        } else {
            "Since this folder is not version controlled, we recommend requiring approval of all edits and commands."
        };

        column.push(
            Paragraph::new(guidance.to_string())
                .wrap(Wrap { trim: true })
                .inset(Insets::tlbr(0, 2, 0, 0)),
        );
        column.push("");

        let mut options: Vec<(&str, TrustDirectorySelection)> = Vec::new();
        if self.is_git_repo {
            options.push((
                "Yes, allow Codex to work in this folder without asking for approval",
                TrustDirectorySelection::Trust,
            ));
            options.push((
                "No, ask me to approve edits and commands",
                TrustDirectorySelection::DontTrust,
            ));
        } else {
            options.push((
                "Allow Codex to work in this folder without asking for approval",
                TrustDirectorySelection::Trust,
            ));
            options.push((
                "Require approval of edits and commands",
                TrustDirectorySelection::DontTrust,
            ));
        }

        for (idx, (text, selection)) in options.iter().enumerate() {
            column.push(selection_option_row(
                idx,
                text.to_string(),
                self.highlighted == *selection,
            ));
        }

        column.push("");

        if let Some(error) = &self.error {
            column.push(
                Paragraph::new(error.to_string())
                    .red()
                    .wrap(Wrap { trim: true })
                    .inset(Insets::tlbr(0, 2, 0, 0)),
            );
            column.push("");
        }

        column.push(
            Line::from(vec![
                "Press ".dim(),
                primary_binding(&self.keymap.trust_confirm)
                    .unwrap_or_else(|| key_hint::plain(KeyCode::Enter))
                    .into(),
                " to continue".dim(),
            ])
            .inset(Insets::tlbr(0, 2, 0, 0)),
        );

        column.render(area, buf);
    }
}

impl KeyboardHandler for TrustDirectoryWidget {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if key_event.kind == KeyEventKind::Release {
            return;
        }

        if self.keymap.trust_up.matches(key_event) {
            self.highlighted = TrustDirectorySelection::Trust;
            return;
        }
        if self.keymap.trust_down.matches(key_event) {
            self.highlighted = TrustDirectorySelection::DontTrust;
            return;
        }
        if self.keymap.trust_select_trust.matches(key_event) {
            self.handle_trust();
            return;
        }
        if self.keymap.trust_select_dont_trust.matches(key_event) {
            self.handle_dont_trust();
            return;
        }
        if self.keymap.trust_confirm.matches(key_event) {
            match self.highlighted {
                TrustDirectorySelection::Trust => self.handle_trust(),
                TrustDirectorySelection::DontTrust => self.handle_dont_trust(),
            }
        }
    }
}

impl StepStateProvider for TrustDirectoryWidget {
    fn get_step_state(&self) -> StepState {
        match self.selection {
            Some(_) => StepState::Complete,
            None => StepState::InProgress,
        }
    }
}

impl TrustDirectoryWidget {
    fn handle_trust(&mut self) {
        let target =
            resolve_root_git_project_for_trust(&self.cwd).unwrap_or_else(|| self.cwd.clone());
        if let Err(e) = set_project_trust_level(&self.codex_home, &target, TrustLevel::Trusted) {
            tracing::error!("Failed to set project trusted: {e:?}");
            self.error = Some(format!("Failed to set trust for {}: {e}", target.display()));
        }

        self.selection = Some(TrustDirectorySelection::Trust);
    }

    fn handle_dont_trust(&mut self) {
        self.highlighted = TrustDirectorySelection::DontTrust;
        let target =
            resolve_root_git_project_for_trust(&self.cwd).unwrap_or_else(|| self.cwd.clone());
        if let Err(e) = set_project_trust_level(&self.codex_home, &target, TrustLevel::Untrusted) {
            tracing::error!("Failed to set project untrusted: {e:?}");
            self.error = Some(format!(
                "Failed to set untrusted for {}: {e}",
                target.display()
            ));
        }

        self.selection = Some(TrustDirectorySelection::DontTrust);
    }
}

#[cfg(test)]
mod tests {
    use crate::test_backend::VT100Backend;

    use super::*;
    use crate::keymap::TuiKeymap;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyEventKind;
    use crossterm::event::KeyModifiers;
    use pretty_assertions::assert_eq;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_keymap() -> Arc<TuiKeymap> {
        Arc::new(TuiKeymap::defaults(false, false))
    }

    #[test]
    fn release_event_does_not_change_selection() {
        let codex_home = TempDir::new().expect("temp home");
        let mut widget = TrustDirectoryWidget {
            codex_home: codex_home.path().to_path_buf(),
            cwd: PathBuf::from("."),
            is_git_repo: false,
            selection: None,
            highlighted: TrustDirectorySelection::DontTrust,
            error: None,
            keymap: test_keymap(),
        };

        let release = KeyEvent {
            kind: KeyEventKind::Release,
            ..KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        };
        widget.handle_key_event(release);
        assert_eq!(widget.selection, None);

        let press = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        widget.handle_key_event(press);
        assert_eq!(widget.selection, Some(TrustDirectorySelection::DontTrust));
    }

    #[test]
    fn renders_snapshot_for_git_repo() {
        let codex_home = TempDir::new().expect("temp home");
        let widget = TrustDirectoryWidget {
            codex_home: codex_home.path().to_path_buf(),
            cwd: PathBuf::from("/workspace/project"),
            is_git_repo: true,
            selection: None,
            highlighted: TrustDirectorySelection::Trust,
            error: None,
            keymap: test_keymap(),
        };

        let mut terminal = Terminal::new(VT100Backend::new(70, 14)).expect("terminal");
        terminal
            .draw(|f| (&widget).render_ref(f.area(), f.buffer_mut()))
            .expect("draw");

        insta::assert_snapshot!(terminal.backend());
    }
}
