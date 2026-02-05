use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;

use codex_core::config::Config;
use codex_core::config::types::KeybindingValue;
use codex_core::config::types::KeybindingsToml;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub key: KeyCode,
    pub modifiers: KeyModifiers,
    pub fallback: Option<KeyCode>,
}

impl KeyChord {
    pub fn matches(&self, event: KeyEvent) -> bool {
        if event.code == self.key {
            if event.modifiers == self.modifiers {
                return true;
            }
            if let KeyCode::Char(ch) = self.key {
                if ch != ' ' {
                    let mut mods = event.modifiers;
                    mods.remove(KeyModifiers::SHIFT);
                    if mods == self.modifiers {
                        return true;
                    }
                }
            }
        }
        if let (KeyCode::Char(expected), KeyCode::Char(actual)) = (self.key, event.code) {
            if self
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
            {
                let mut mods = event.modifiers;
                mods.remove(KeyModifiers::SHIFT);
                if mods == self.modifiers && expected.eq_ignore_ascii_case(&actual) {
                    return true;
                }
            }
        }
        if self.key == KeyCode::Tab
            && self.modifiers == KeyModifiers::SHIFT
            && event.code == KeyCode::BackTab
        {
            return true;
        }
        if self.key == KeyCode::BackTab
            && self.modifiers == KeyModifiers::NONE
            && event.code == KeyCode::Tab
            && event.modifiers == KeyModifiers::SHIFT
        {
            return true;
        }
        if let Some(fallback) = self.fallback {
            if event.code == fallback && event.modifiers == KeyModifiers::NONE {
                return true;
            }
        }
        false
    }

    fn conflict_keys(&self) -> Vec<(KeyModifiers, KeyCode)> {
        let mut keys = HashSet::new();
        let mut insert = |mods, key| {
            keys.insert((mods, key));
        };

        insert(self.modifiers, self.key);

        match self.key {
            KeyCode::Char(ch) => {
                if !self.modifiers.contains(KeyModifiers::SHIFT)
                    && self
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
                {
                    for variant in ascii_case_variants(ch) {
                        insert(self.modifiers, KeyCode::Char(variant));
                        insert(self.modifiers | KeyModifiers::SHIFT, KeyCode::Char(variant));
                    }
                }
            }
            KeyCode::Tab => {
                if self.modifiers == KeyModifiers::SHIFT {
                    for mods in all_modifier_variants() {
                        insert(mods, KeyCode::BackTab);
                    }
                }
            }
            KeyCode::BackTab => {
                if self.modifiers == KeyModifiers::NONE {
                    insert(KeyModifiers::SHIFT, KeyCode::Tab);
                }
            }
            _ => {}
        }

        if let Some(fallback) = self.fallback {
            insert(KeyModifiers::NONE, fallback);
        }

        keys.into_iter().collect()
    }
}

fn ascii_case_variants(ch: char) -> [char; 2] {
    [ch.to_ascii_lowercase(), ch.to_ascii_uppercase()]
}

fn all_modifier_variants() -> Vec<KeyModifiers> {
    const FLAGS: [KeyModifiers; 4] = [
        KeyModifiers::SHIFT,
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SUPER,
    ];
    let mut variants = Vec::new();
    for mask in 0..(1 << FLAGS.len()) {
        let mut mods = KeyModifiers::NONE;
        for (idx, flag) in FLAGS.iter().enumerate() {
            if mask & (1 << idx) != 0 {
                mods |= *flag;
            }
        }
        variants.push(mods);
    }
    variants
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn keybindings(pairs: &[(&str, &str)]) -> KeybindingsToml {
        let mut map = HashMap::new();
        for (action, chord) in pairs {
            map.insert(
                (*action).to_string(),
                KeybindingValue::Single((*chord).to_string()),
            );
        }
        KeybindingsToml(map)
    }

    fn assert_conflict(err: KeymapError, context: &str, expected_actions: &[&str]) {
        match err {
            KeymapError::Conflict {
                context: actual_context,
                actions,
                ..
            } => {
                assert_eq!(actual_context, context);
                let actual: HashSet<String> = actions.into_iter().collect();
                let expected: HashSet<String> = expected_actions
                    .iter()
                    .map(|action| (*action).to_string())
                    .collect();
                assert_eq!(actual, expected);
            }
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn conflicts_detect_ctrl_case_insensitive_chords() {
        let keybindings = keybindings(&[
            ("chat_quit_or_interrupt_primary", "Ctrl+C"),
            ("chat_quit_or_interrupt_secondary", "Ctrl+c"),
        ]);

        let err = TuiKeymap::from_keybindings(Some(&keybindings), false, false)
            .expect_err("should detect conflict for ctrl case variants");

        assert_conflict(
            err,
            "chat",
            &[
                "chat_quit_or_interrupt_primary",
                "chat_quit_or_interrupt_secondary",
            ],
        );
    }

    #[test]
    fn conflicts_detect_shift_tab_backtab_equivalence() {
        let keybindings =
            keybindings(&[("popup_accept", "Shift+Tab"), ("popup_cancel", "BackTab")]);

        let err = TuiKeymap::from_keybindings(Some(&keybindings), false, false)
            .expect_err("should detect conflict for shift-tab/backtab");

        assert_conflict(err, "popup", &["popup_accept", "popup_cancel"]);
    }

    #[test]
    fn conflicts_detect_ctrl_char_fallbacks() {
        let keybindings =
            keybindings(&[("text_line_start", "Ctrl+A"), ("text_line_end", "\u{0001}")]);

        let err = TuiKeymap::from_keybindings(Some(&keybindings), false, false)
            .expect_err("should detect conflict for ctrl-char fallback");

        assert_conflict(err, "text", &["text_line_start", "text_line_end"]);
    }

    #[test]
    fn space_and_shift_space_are_distinct() {
        let space = parse_key_chord("Space").expect("space should parse");
        let shift_space = parse_key_chord("Shift+Space").expect("shift+space should parse");

        assert!(space.matches(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert!(!space.matches(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT)));
        assert!(shift_space.matches(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT)));
        assert!(!shift_space.matches(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)));
    }
}

#[derive(Clone, Debug, Default)]
pub struct KeyBindingSet(pub Vec<KeyChord>);

impl KeyBindingSet {
    pub fn matches(&self, event: KeyEvent) -> bool {
        self.0.iter().any(|ch| ch.matches(event))
    }

    pub fn match_chord(&self, event: KeyEvent) -> Option<&KeyChord> {
        self.0.iter().find(|ch| ch.matches(event))
    }
}

#[derive(Clone, Debug)]
pub struct TextAreaKeymap {
    pub newline: KeyBindingSet,
    pub backspace: KeyBindingSet,
    pub delete_forward: KeyBindingSet,
    pub delete_backward_word: KeyBindingSet,
    pub delete_forward_word: KeyBindingSet,
    pub kill_line_start: KeyBindingSet,
    pub kill_line_end: KeyBindingSet,
    pub yank: KeyBindingSet,
    pub move_left: KeyBindingSet,
    pub move_right: KeyBindingSet,
    pub move_up: KeyBindingSet,
    pub move_down: KeyBindingSet,
    pub word_left: KeyBindingSet,
    pub word_right: KeyBindingSet,
    pub line_start: KeyBindingSet,
    pub line_end: KeyBindingSet,
}

impl TextAreaKeymap {
    pub fn from_tui(tui: &TuiKeymap) -> Self {
        Self {
            newline: tui.text_newline.clone(),
            backspace: tui.text_backspace.clone(),
            delete_forward: tui.text_delete_forward.clone(),
            delete_backward_word: tui.text_delete_backward_word.clone(),
            delete_forward_word: tui.text_delete_forward_word.clone(),
            kill_line_start: tui.text_kill_line_start.clone(),
            kill_line_end: tui.text_kill_line_end.clone(),
            yank: tui.text_yank.clone(),
            move_left: tui.text_move_left.clone(),
            move_right: tui.text_move_right.clone(),
            move_up: tui.text_move_up.clone(),
            move_down: tui.text_move_down.clone(),
            word_left: tui.text_word_left.clone(),
            word_right: tui.text_word_right.clone(),
            line_start: tui.text_line_start.clone(),
            line_end: tui.text_line_end.clone(),
        }
    }

    pub fn default_for_text() -> Self {
        let defaults = default_bindings(true, false);
        Self {
            newline: parse_default_set(&defaults, "text_newline"),
            backspace: parse_default_set(&defaults, "text_backspace"),
            delete_forward: parse_default_set(&defaults, "text_delete_forward"),
            delete_backward_word: parse_default_set(&defaults, "text_delete_backward_word"),
            delete_forward_word: parse_default_set(&defaults, "text_delete_forward_word"),
            kill_line_start: parse_default_set(&defaults, "text_kill_line_start"),
            kill_line_end: parse_default_set(&defaults, "text_kill_line_end"),
            yank: parse_default_set(&defaults, "text_yank"),
            move_left: parse_default_set(&defaults, "text_move_left"),
            move_right: parse_default_set(&defaults, "text_move_right"),
            move_up: parse_default_set(&defaults, "text_move_up"),
            move_down: parse_default_set(&defaults, "text_move_down"),
            word_left: parse_default_set(&defaults, "text_word_left"),
            word_right: parse_default_set(&defaults, "text_word_right"),
            line_start: parse_default_set(&defaults, "text_line_start"),
            line_end: parse_default_set(&defaults, "text_line_end"),
        }
    }
}

fn parse_default_set(defaults: &HashMap<&'static str, Vec<String>>, key: &str) -> KeyBindingSet {
    let bindings = defaults.get(key).expect("default keybinding is missing");
    let mut chords = Vec::new();
    for binding in bindings {
        let chord = parse_key_chord(binding).expect("default keybinding should parse");
        chords.push(chord);
    }
    KeyBindingSet(chords)
}

#[derive(Debug, Clone)]
pub struct TuiKeymap {
    pub global_suspend: KeyBindingSet,
    pub global_show_transcript: KeyBindingSet,
    pub global_external_editor: KeyBindingSet,
    pub global_backtrack_prime: KeyBindingSet,
    pub global_backtrack_confirm: KeyBindingSet,

    pub chat_quit_or_interrupt_primary: KeyBindingSet,
    pub chat_quit_or_interrupt_secondary: KeyBindingSet,
    pub chat_paste_image: KeyBindingSet,
    pub chat_recall_queued_message: KeyBindingSet,
    pub chat_change_mode: KeyBindingSet,

    pub composer_submit: KeyBindingSet,
    pub composer_queue: KeyBindingSet,
    pub composer_newline: KeyBindingSet,
    pub composer_toggle_shortcuts: KeyBindingSet,

    pub popup_up: KeyBindingSet,
    pub popup_down: KeyBindingSet,
    pub popup_accept: KeyBindingSet,
    pub popup_cancel: KeyBindingSet,

    pub text_newline: KeyBindingSet,
    pub text_backspace: KeyBindingSet,
    pub text_delete_forward: KeyBindingSet,
    pub text_delete_backward_word: KeyBindingSet,
    pub text_delete_forward_word: KeyBindingSet,
    pub text_kill_line_start: KeyBindingSet,
    pub text_kill_line_end: KeyBindingSet,
    pub text_yank: KeyBindingSet,
    pub text_move_left: KeyBindingSet,
    pub text_move_right: KeyBindingSet,
    pub text_move_up: KeyBindingSet,
    pub text_move_down: KeyBindingSet,
    pub text_word_left: KeyBindingSet,
    pub text_word_right: KeyBindingSet,
    pub text_line_start: KeyBindingSet,
    pub text_line_end: KeyBindingSet,

    pub pager_scroll_up: KeyBindingSet,
    pub pager_scroll_down: KeyBindingSet,
    pub pager_page_up: KeyBindingSet,
    pub pager_page_down: KeyBindingSet,
    pub pager_half_page_up: KeyBindingSet,
    pub pager_half_page_down: KeyBindingSet,
    pub pager_jump_top: KeyBindingSet,
    pub pager_jump_bottom: KeyBindingSet,
    pub pager_quit: KeyBindingSet,
    pub pager_backtrack_prev: KeyBindingSet,
    pub pager_backtrack_next: KeyBindingSet,
    pub pager_backtrack_confirm: KeyBindingSet,

    pub backtrack_overlay_prev: KeyBindingSet,
    pub backtrack_overlay_next: KeyBindingSet,
    pub backtrack_overlay_confirm: KeyBindingSet,

    pub rui_cancel: KeyBindingSet,
    pub rui_next_question: KeyBindingSet,
    pub rui_prev_question: KeyBindingSet,
    pub rui_option_up: KeyBindingSet,
    pub rui_option_down: KeyBindingSet,
    pub rui_option_select: KeyBindingSet,
    pub rui_option_clear: KeyBindingSet,
    pub rui_option_to_notes: KeyBindingSet,
    pub rui_submit_or_next: KeyBindingSet,
    pub rui_notes_to_options: KeyBindingSet,
    pub rui_notes_backspace_empty: KeyBindingSet,

    pub list_up: KeyBindingSet,
    pub list_down: KeyBindingSet,
    pub list_search_backspace: KeyBindingSet,
    pub list_cancel: KeyBindingSet,
    pub list_accept: KeyBindingSet,
    pub list_pick_index: KeyBindingSet,

    pub approval_approve: KeyBindingSet,
    pub approval_approve_policy: KeyBindingSet,
    pub approval_approve_session: KeyBindingSet,
    pub approval_reject: KeyBindingSet,
    pub approval_cancel: KeyBindingSet,

    pub skills_up: KeyBindingSet,
    pub skills_down: KeyBindingSet,
    pub skills_toggle: KeyBindingSet,
    pub skills_search_backspace: KeyBindingSet,
    pub skills_cancel: KeyBindingSet,

    pub features_up: KeyBindingSet,
    pub features_down: KeyBindingSet,
    pub features_toggle: KeyBindingSet,
    pub features_cancel: KeyBindingSet,

    pub resume_exit: KeyBindingSet,
    pub resume_start_fresh: KeyBindingSet,
    pub resume_accept: KeyBindingSet,
    pub resume_up: KeyBindingSet,
    pub resume_down: KeyBindingSet,
    pub resume_page_up: KeyBindingSet,
    pub resume_page_down: KeyBindingSet,
    pub resume_search_backspace: KeyBindingSet,

    pub update_exit: KeyBindingSet,
    pub update_up: KeyBindingSet,
    pub update_down: KeyBindingSet,
    pub update_select_1: KeyBindingSet,
    pub update_select_2: KeyBindingSet,
    pub update_select_3: KeyBindingSet,
    pub update_confirm: KeyBindingSet,
    pub update_cancel: KeyBindingSet,

    pub migration_exit: KeyBindingSet,
    pub migration_up: KeyBindingSet,
    pub migration_down: KeyBindingSet,
    pub migration_select_1: KeyBindingSet,
    pub migration_select_2: KeyBindingSet,
    pub migration_confirm: KeyBindingSet,

    pub oss_cancel: KeyBindingSet,
    pub oss_left: KeyBindingSet,
    pub oss_right: KeyBindingSet,
    pub oss_confirm: KeyBindingSet,
    pub oss_default: KeyBindingSet,
    pub oss_select_l: KeyBindingSet,
    pub oss_select_o: KeyBindingSet,
    pub oss_select_c: KeyBindingSet,

    pub cwd_exit: KeyBindingSet,
    pub cwd_up: KeyBindingSet,
    pub cwd_down: KeyBindingSet,
    pub cwd_select_session: KeyBindingSet,
    pub cwd_select_current: KeyBindingSet,
    pub cwd_confirm: KeyBindingSet,

    pub onboarding_exit: KeyBindingSet,
    pub onboarding_quit: KeyBindingSet,
    pub welcome_cycle_animation: KeyBindingSet,

    pub auth_up: KeyBindingSet,
    pub auth_down: KeyBindingSet,
    pub auth_select_1: KeyBindingSet,
    pub auth_select_2: KeyBindingSet,
    pub auth_select_3: KeyBindingSet,
    pub auth_confirm: KeyBindingSet,
    pub auth_back: KeyBindingSet,

    pub auth_api_key_submit: KeyBindingSet,
    pub auth_api_key_back: KeyBindingSet,
    pub auth_api_key_backspace: KeyBindingSet,

    pub trust_up: KeyBindingSet,
    pub trust_down: KeyBindingSet,
    pub trust_select_trust: KeyBindingSet,
    pub trust_select_dont_trust: KeyBindingSet,
    pub trust_confirm: KeyBindingSet,
}

#[derive(Debug, Clone)]
pub enum KeymapError {
    UnknownAction(String),
    InvalidChord {
        action: String,
        chord: String,
        message: String,
    },
    Conflict {
        context: &'static str,
        chord: String,
        actions: Vec<String>,
    },
}

impl fmt::Display for KeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeymapError::UnknownAction(a) => write!(f, "unknown keybinding action '{a}'"),
            KeymapError::InvalidChord {
                action,
                chord,
                message,
            } => write!(f, "invalid keybinding '{chord}' for '{action}': {message}"),
            KeymapError::Conflict {
                context,
                chord,
                actions,
            } => write!(
                f,
                "keybinding conflict in {context}: {chord} used by {}",
                format_action_list(actions)
            ),
        }
    }
}

impl TuiKeymap {
    pub fn defaults(enhanced_keys_supported: bool, is_wsl: bool) -> Self {
        let defaults = default_bindings(enhanced_keys_supported, is_wsl);
        let merged = defaults
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        build_keymap(merged).expect("default keybindings should be valid")
    }

    pub fn from_config(
        config: &Config,
        enhanced_keys_supported: bool,
        is_wsl: bool,
    ) -> Result<Self, KeymapError> {
        Self::from_keybindings(
            config.tui_keybindings.as_ref(),
            enhanced_keys_supported,
            is_wsl,
        )
    }

    pub fn from_keybindings(
        keybindings: Option<&KeybindingsToml>,
        enhanced_keys_supported: bool,
        is_wsl: bool,
    ) -> Result<Self, KeymapError> {
        let defaults = default_bindings(enhanced_keys_supported, is_wsl);
        let action_set: HashSet<&'static str> = TUI_KEYBINDING_ACTIONS.iter().copied().collect();

        if let Some(tui) = keybindings {
            for action in tui.0.keys() {
                if !action_set.contains(action.as_str()) {
                    return Err(KeymapError::UnknownAction(action.clone()));
                }
            }
        }

        let mut merged: HashMap<String, Vec<String>> = defaults
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();

        if let Some(tui) = keybindings {
            for (action, value) in &tui.0 {
                match value {
                    KeybindingValue::Single(s) => {
                        merged.insert(action.clone(), vec![s.clone()]);
                    }
                    KeybindingValue::Multiple(v) => {
                        merged.insert(action.clone(), v.clone());
                    }
                };
            }
        }

        build_keymap(merged)
    }
}

fn take(map: &mut HashMap<String, KeyBindingSet>, key: &str) -> Result<KeyBindingSet, KeymapError> {
    map.remove(key)
        .ok_or_else(|| KeymapError::UnknownAction(key.to_string()))
}

fn build_keymap(merged: HashMap<String, Vec<String>>) -> Result<TuiKeymap, KeymapError> {
    let mut parsed: HashMap<String, KeyBindingSet> = HashMap::new();
    for (action, bindings) in merged {
        let mut chords = Vec::new();
        for chord in bindings {
            let parsed_chord =
                parse_key_chord(&chord).map_err(|message| KeymapError::InvalidChord {
                    action: action.clone(),
                    chord: chord.clone(),
                    message,
                })?;
            chords.push(parsed_chord);
        }
        parsed.insert(action, KeyBindingSet(chords));
    }

    let keymap = TuiKeymap {
        global_suspend: take(&mut parsed, "global_suspend")?,
        global_show_transcript: take(&mut parsed, "global_show_transcript")?,
        global_external_editor: take(&mut parsed, "global_external_editor")?,
        global_backtrack_prime: take(&mut parsed, "global_backtrack_prime")?,
        global_backtrack_confirm: take(&mut parsed, "global_backtrack_confirm")?,

        chat_quit_or_interrupt_primary: take(&mut parsed, "chat_quit_or_interrupt_primary")?,
        chat_quit_or_interrupt_secondary: take(&mut parsed, "chat_quit_or_interrupt_secondary")?,
        chat_paste_image: take(&mut parsed, "chat_paste_image")?,
        chat_recall_queued_message: take(&mut parsed, "chat_recall_queued_message")?,
        chat_change_mode: take(&mut parsed, "chat_change_mode")?,

        composer_submit: take(&mut parsed, "composer_submit")?,
        composer_queue: take(&mut parsed, "composer_queue")?,
        composer_newline: take(&mut parsed, "composer_newline")?,
        composer_toggle_shortcuts: take(&mut parsed, "composer_toggle_shortcuts")?,

        popup_up: take(&mut parsed, "popup_up")?,
        popup_down: take(&mut parsed, "popup_down")?,
        popup_accept: take(&mut parsed, "popup_accept")?,
        popup_cancel: take(&mut parsed, "popup_cancel")?,

        text_newline: take(&mut parsed, "text_newline")?,
        text_backspace: take(&mut parsed, "text_backspace")?,
        text_delete_forward: take(&mut parsed, "text_delete_forward")?,
        text_delete_backward_word: take(&mut parsed, "text_delete_backward_word")?,
        text_delete_forward_word: take(&mut parsed, "text_delete_forward_word")?,
        text_kill_line_start: take(&mut parsed, "text_kill_line_start")?,
        text_kill_line_end: take(&mut parsed, "text_kill_line_end")?,
        text_yank: take(&mut parsed, "text_yank")?,
        text_move_left: take(&mut parsed, "text_move_left")?,
        text_move_right: take(&mut parsed, "text_move_right")?,
        text_move_up: take(&mut parsed, "text_move_up")?,
        text_move_down: take(&mut parsed, "text_move_down")?,
        text_word_left: take(&mut parsed, "text_word_left")?,
        text_word_right: take(&mut parsed, "text_word_right")?,
        text_line_start: take(&mut parsed, "text_line_start")?,
        text_line_end: take(&mut parsed, "text_line_end")?,

        pager_scroll_up: take(&mut parsed, "pager_scroll_up")?,
        pager_scroll_down: take(&mut parsed, "pager_scroll_down")?,
        pager_page_up: take(&mut parsed, "pager_page_up")?,
        pager_page_down: take(&mut parsed, "pager_page_down")?,
        pager_half_page_up: take(&mut parsed, "pager_half_page_up")?,
        pager_half_page_down: take(&mut parsed, "pager_half_page_down")?,
        pager_jump_top: take(&mut parsed, "pager_jump_top")?,
        pager_jump_bottom: take(&mut parsed, "pager_jump_bottom")?,
        pager_quit: take(&mut parsed, "pager_quit")?,
        pager_backtrack_prev: take(&mut parsed, "pager_backtrack_prev")?,
        pager_backtrack_next: take(&mut parsed, "pager_backtrack_next")?,
        pager_backtrack_confirm: take(&mut parsed, "pager_backtrack_confirm")?,

        backtrack_overlay_prev: take(&mut parsed, "backtrack_overlay_prev")?,
        backtrack_overlay_next: take(&mut parsed, "backtrack_overlay_next")?,
        backtrack_overlay_confirm: take(&mut parsed, "backtrack_overlay_confirm")?,

        rui_cancel: take(&mut parsed, "rui_cancel")?,
        rui_next_question: take(&mut parsed, "rui_next_question")?,
        rui_prev_question: take(&mut parsed, "rui_prev_question")?,
        rui_option_up: take(&mut parsed, "rui_option_up")?,
        rui_option_down: take(&mut parsed, "rui_option_down")?,
        rui_option_select: take(&mut parsed, "rui_option_select")?,
        rui_option_clear: take(&mut parsed, "rui_option_clear")?,
        rui_option_to_notes: take(&mut parsed, "rui_option_to_notes")?,
        rui_submit_or_next: take(&mut parsed, "rui_submit_or_next")?,
        rui_notes_to_options: take(&mut parsed, "rui_notes_to_options")?,
        rui_notes_backspace_empty: take(&mut parsed, "rui_notes_backspace_empty")?,

        list_up: take(&mut parsed, "list_up")?,
        list_down: take(&mut parsed, "list_down")?,
        list_search_backspace: take(&mut parsed, "list_search_backspace")?,
        list_cancel: take(&mut parsed, "list_cancel")?,
        list_accept: take(&mut parsed, "list_accept")?,
        list_pick_index: take(&mut parsed, "list_pick_index")?,

        approval_approve: take(&mut parsed, "approval_approve")?,
        approval_approve_policy: take(&mut parsed, "approval_approve_policy")?,
        approval_approve_session: take(&mut parsed, "approval_approve_session")?,
        approval_reject: take(&mut parsed, "approval_reject")?,
        approval_cancel: take(&mut parsed, "approval_cancel")?,

        skills_up: take(&mut parsed, "skills_up")?,
        skills_down: take(&mut parsed, "skills_down")?,
        skills_toggle: take(&mut parsed, "skills_toggle")?,
        skills_search_backspace: take(&mut parsed, "skills_search_backspace")?,
        skills_cancel: take(&mut parsed, "skills_cancel")?,

        features_up: take(&mut parsed, "features_up")?,
        features_down: take(&mut parsed, "features_down")?,
        features_toggle: take(&mut parsed, "features_toggle")?,
        features_cancel: take(&mut parsed, "features_cancel")?,

        resume_exit: take(&mut parsed, "resume_exit")?,
        resume_start_fresh: take(&mut parsed, "resume_start_fresh")?,
        resume_accept: take(&mut parsed, "resume_accept")?,
        resume_up: take(&mut parsed, "resume_up")?,
        resume_down: take(&mut parsed, "resume_down")?,
        resume_page_up: take(&mut parsed, "resume_page_up")?,
        resume_page_down: take(&mut parsed, "resume_page_down")?,
        resume_search_backspace: take(&mut parsed, "resume_search_backspace")?,

        update_exit: take(&mut parsed, "update_exit")?,
        update_up: take(&mut parsed, "update_up")?,
        update_down: take(&mut parsed, "update_down")?,
        update_select_1: take(&mut parsed, "update_select_1")?,
        update_select_2: take(&mut parsed, "update_select_2")?,
        update_select_3: take(&mut parsed, "update_select_3")?,
        update_confirm: take(&mut parsed, "update_confirm")?,
        update_cancel: take(&mut parsed, "update_cancel")?,

        migration_exit: take(&mut parsed, "migration_exit")?,
        migration_up: take(&mut parsed, "migration_up")?,
        migration_down: take(&mut parsed, "migration_down")?,
        migration_select_1: take(&mut parsed, "migration_select_1")?,
        migration_select_2: take(&mut parsed, "migration_select_2")?,
        migration_confirm: take(&mut parsed, "migration_confirm")?,

        oss_cancel: take(&mut parsed, "oss_cancel")?,
        oss_left: take(&mut parsed, "oss_left")?,
        oss_right: take(&mut parsed, "oss_right")?,
        oss_confirm: take(&mut parsed, "oss_confirm")?,
        oss_default: take(&mut parsed, "oss_default")?,
        oss_select_l: take(&mut parsed, "oss_select_l")?,
        oss_select_o: take(&mut parsed, "oss_select_o")?,
        oss_select_c: take(&mut parsed, "oss_select_c")?,

        cwd_exit: take(&mut parsed, "cwd_exit")?,
        cwd_up: take(&mut parsed, "cwd_up")?,
        cwd_down: take(&mut parsed, "cwd_down")?,
        cwd_select_session: take(&mut parsed, "cwd_select_session")?,
        cwd_select_current: take(&mut parsed, "cwd_select_current")?,
        cwd_confirm: take(&mut parsed, "cwd_confirm")?,

        onboarding_exit: take(&mut parsed, "onboarding_exit")?,
        onboarding_quit: take(&mut parsed, "onboarding_quit")?,
        welcome_cycle_animation: take(&mut parsed, "welcome_cycle_animation")?,

        auth_up: take(&mut parsed, "auth_up")?,
        auth_down: take(&mut parsed, "auth_down")?,
        auth_select_1: take(&mut parsed, "auth_select_1")?,
        auth_select_2: take(&mut parsed, "auth_select_2")?,
        auth_select_3: take(&mut parsed, "auth_select_3")?,
        auth_confirm: take(&mut parsed, "auth_confirm")?,
        auth_back: take(&mut parsed, "auth_back")?,

        auth_api_key_submit: take(&mut parsed, "auth_api_key_submit")?,
        auth_api_key_back: take(&mut parsed, "auth_api_key_back")?,
        auth_api_key_backspace: take(&mut parsed, "auth_api_key_backspace")?,

        trust_up: take(&mut parsed, "trust_up")?,
        trust_down: take(&mut parsed, "trust_down")?,
        trust_select_trust: take(&mut parsed, "trust_select_trust")?,
        trust_select_dont_trust: take(&mut parsed, "trust_select_dont_trust")?,
        trust_confirm: take(&mut parsed, "trust_confirm")?,
    };

    validate_conflicts(&keymap)?;

    Ok(keymap)
}

fn parse_key_chord(input: &str) -> Result<KeyChord, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err("empty key chord".to_string());
    }

    let parts: Vec<&str> = raw.split('+').map(|s| s.trim()).collect();
    if parts.is_empty() {
        return Err("empty key chord".to_string());
    }

    let mut modifiers = KeyModifiers::NONE;
    let mut key: Option<KeyCode> = None;

    for part in parts {
        if part.is_empty() {
            return Err("empty chord segment".to_string());
        }
        let lower = part.to_ascii_lowercase();

        match lower.as_str() {
            "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
            "alt" | "option" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            "meta" | "cmd" | "command" | "super" => modifiers |= KeyModifiers::SUPER,
            _ => {
                if key.is_some() {
                    return Err("multiple non-modifier keys in chord".to_string());
                }
                key = Some(parse_keycode(part)?);
            }
        }
    }

    let key = key.ok_or_else(|| "missing key in chord".to_string())?;
    let fallback = ctrl_char_fallback(key, modifiers);

    Ok(KeyChord {
        key,
        modifiers,
        fallback,
    })
}

fn parse_keycode(part: &str) -> Result<KeyCode, String> {
    let lower = part.to_ascii_lowercase();
    match lower.as_str() {
        "enter" | "return" => Ok(KeyCode::Enter),
        "esc" | "escape" => Ok(KeyCode::Esc),
        "tab" => Ok(KeyCode::Tab),
        "backtab" => Ok(KeyCode::BackTab),
        "backspace" => Ok(KeyCode::Backspace),
        "delete" => Ok(KeyCode::Delete),
        "insert" => Ok(KeyCode::Insert),
        "space" => Ok(KeyCode::Char(' ')),
        "left" => Ok(KeyCode::Left),
        "right" => Ok(KeyCode::Right),
        "up" => Ok(KeyCode::Up),
        "down" => Ok(KeyCode::Down),
        "home" => Ok(KeyCode::Home),
        "end" => Ok(KeyCode::End),
        "pageup" | "pgup" => Ok(KeyCode::PageUp),
        "pagedown" | "pgdn" => Ok(KeyCode::PageDown),
        _ => {
            if let Some(fkey) = parse_function_key(&lower) {
                return Ok(fkey);
            }
            if let Some(ch) = parse_named_punct(&lower) {
                return Ok(KeyCode::Char(ch));
            }
            if part.chars().count() == 1 {
                let ch = part.chars().next().unwrap();
                return Ok(KeyCode::Char(ch));
            }
            Err(format!("unknown key '{part}'"))
        }
    }
}

fn parse_function_key(lower: &str) -> Option<KeyCode> {
    if let Some(num) = lower.strip_prefix('f') {
        if let Ok(n) = num.parse::<u8>() {
            return Some(KeyCode::F(n));
        }
    }
    None
}

fn parse_named_punct(lower: &str) -> Option<char> {
    match lower {
        "plus" => Some('+'),
        "minus" => Some('-'),
        "comma" => Some(','),
        "period" | "dot" => Some('.'),
        "slash" => Some('/'),
        "backslash" => Some('\\'),
        "quote" => Some('\''),
        "doublequote" => Some('"'),
        "semicolon" => Some(';'),
        _ => None,
    }
}

fn format_action_list(actions: &[String]) -> String {
    match actions.len() {
        0 => String::new(),
        1 => actions[0].clone(),
        2 => format!("{} and {}", actions[0], actions[1]),
        _ => {
            let mut out = String::new();
            for (idx, action) in actions.iter().enumerate() {
                if idx == 0 {
                    out.push_str(action);
                    continue;
                }
                if idx == actions.len() - 1 {
                    out.push_str(", and ");
                } else {
                    out.push_str(", ");
                }
                out.push_str(action);
            }
            out
        }
    }
}

fn format_keybinding(modifiers: KeyModifiers, key: KeyCode) -> String {
    if modifiers == KeyModifiers::NONE {
        if let KeyCode::Char(code) = key {
            if let Some(letter) = ctrl_char_to_letter(code) {
                return format!("Ctrl+{letter}");
            }
        }
    }
    let mut parts = Vec::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("Ctrl".to_string());
    }
    if modifiers.contains(KeyModifiers::ALT) {
        parts.push("Alt".to_string());
    }
    if modifiers.contains(KeyModifiers::SHIFT) {
        parts.push("Shift".to_string());
    }
    if modifiers.contains(KeyModifiers::SUPER) {
        parts.push("Super".to_string());
    }
    parts.push(format_keycode_display(key));
    parts.join("+")
}

fn ctrl_char_to_letter(code: char) -> Option<char> {
    let value = code as u32;
    if (1..=26).contains(&value) {
        let letter = (value as u8 + b'@') as char;
        return Some(letter);
    }
    None
}

fn format_keycode_display(key: KeyCode) -> String {
    match key {
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Backtab".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => {
            if c.is_ascii_alphabetic() {
                c.to_ascii_uppercase().to_string()
            } else {
                c.to_string()
            }
        }
        KeyCode::F(num) => format!("F{num}"),
        _ => format!("{key:?}"),
    }
}

fn ctrl_char_fallback(key: KeyCode, modifiers: KeyModifiers) -> Option<KeyCode> {
    if !modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    if let KeyCode::Char(c) = key {
        if c.is_ascii_alphabetic() {
            let upper = c.to_ascii_uppercase() as u8;
            let code = (upper - b'@') as char;
            return Some(KeyCode::Char(code));
        }
    }
    None
}

fn default_bindings(
    enhanced_keys_supported: bool,
    is_wsl: bool,
) -> HashMap<&'static str, Vec<String>> {
    let mut m = HashMap::new();

    m.insert("global_suspend", vec!["Ctrl+Z".to_string()]);
    m.insert("global_show_transcript", vec!["Ctrl+T".to_string()]);
    m.insert("global_external_editor", vec!["Ctrl+G".to_string()]);
    m.insert("global_backtrack_prime", vec!["Esc".to_string()]);
    m.insert("global_backtrack_confirm", vec!["Enter".to_string()]);

    m.insert("chat_quit_or_interrupt_primary", vec!["Ctrl+C".to_string()]);
    m.insert(
        "chat_quit_or_interrupt_secondary",
        vec!["Ctrl+D".to_string()],
    );
    m.insert(
        "chat_paste_image",
        vec![if is_wsl { "Ctrl+Alt+V" } else { "Ctrl+V" }.to_string()],
    );
    m.insert("chat_recall_queued_message", vec!["Alt+Up".to_string()]);
    m.insert("chat_change_mode", vec!["Shift+Tab".to_string()]);

    m.insert("composer_submit", vec!["Enter".to_string()]);
    m.insert("composer_queue", vec!["Tab".to_string()]);
    m.insert(
        "composer_newline",
        vec![
            if enhanced_keys_supported {
                "Shift+Enter"
            } else {
                "Ctrl+J"
            }
            .to_string(),
        ],
    );
    m.insert("composer_toggle_shortcuts", vec!["?".to_string()]);

    m.insert("popup_up", vec!["Up".to_string(), "Ctrl+P".to_string()]);
    m.insert("popup_down", vec!["Down".to_string(), "Ctrl+N".to_string()]);
    m.insert("popup_accept", vec!["Enter".to_string(), "Tab".to_string()]);
    m.insert("popup_cancel", vec!["Esc".to_string()]);

    m.insert(
        "text_newline",
        vec![
            "Enter".to_string(),
            "Ctrl+J".to_string(),
            "Ctrl+M".to_string(),
        ],
    );
    m.insert(
        "text_backspace",
        vec!["Backspace".to_string(), "Ctrl+H".to_string()],
    );
    m.insert(
        "text_delete_forward",
        vec!["Delete".to_string(), "Ctrl+D".to_string()],
    );
    m.insert(
        "text_delete_backward_word",
        vec![
            "Alt+Backspace".to_string(),
            "Ctrl+W".to_string(),
            "Ctrl+Alt+H".to_string(),
        ],
    );
    m.insert("text_delete_forward_word", vec!["Alt+Delete".to_string()]);
    m.insert("text_kill_line_start", vec!["Ctrl+U".to_string()]);
    m.insert("text_kill_line_end", vec!["Ctrl+K".to_string()]);
    m.insert("text_yank", vec!["Ctrl+Y".to_string()]);
    m.insert(
        "text_move_left",
        vec!["Left".to_string(), "Ctrl+B".to_string()],
    );
    m.insert(
        "text_move_right",
        vec!["Right".to_string(), "Ctrl+F".to_string()],
    );
    m.insert("text_move_up", vec!["Up".to_string(), "Ctrl+P".to_string()]);
    m.insert(
        "text_move_down",
        vec!["Down".to_string(), "Ctrl+N".to_string()],
    );
    m.insert(
        "text_word_left",
        vec![
            "Alt+Left".to_string(),
            "Ctrl+Left".to_string(),
            "Alt+B".to_string(),
        ],
    );
    m.insert(
        "text_word_right",
        vec![
            "Alt+Right".to_string(),
            "Ctrl+Right".to_string(),
            "Alt+F".to_string(),
        ],
    );
    m.insert(
        "text_line_start",
        vec!["Home".to_string(), "Ctrl+A".to_string()],
    );
    m.insert(
        "text_line_end",
        vec!["End".to_string(), "Ctrl+E".to_string()],
    );

    m.insert("pager_scroll_up", vec!["Up".to_string(), "k".to_string()]);
    m.insert(
        "pager_scroll_down",
        vec!["Down".to_string(), "j".to_string()],
    );
    m.insert(
        "pager_page_up",
        vec![
            "PageUp".to_string(),
            "Shift+Space".to_string(),
            "Ctrl+B".to_string(),
        ],
    );
    m.insert(
        "pager_page_down",
        vec![
            "PageDown".to_string(),
            "Space".to_string(),
            "Ctrl+F".to_string(),
        ],
    );
    m.insert("pager_half_page_up", vec!["Ctrl+U".to_string()]);
    m.insert("pager_half_page_down", vec!["Ctrl+D".to_string()]);
    m.insert("pager_jump_top", vec!["Home".to_string()]);
    m.insert("pager_jump_bottom", vec!["End".to_string()]);
    m.insert(
        "pager_quit",
        vec!["q".to_string(), "Ctrl+C".to_string(), "Ctrl+T".to_string()],
    );
    m.insert(
        "pager_backtrack_prev",
        vec!["Esc".to_string(), "Left".to_string()],
    );
    m.insert("pager_backtrack_next", vec!["Right".to_string()]);
    m.insert("pager_backtrack_confirm", vec!["Enter".to_string()]);

    m.insert(
        "backtrack_overlay_prev",
        vec!["Esc".to_string(), "Left".to_string()],
    );
    m.insert("backtrack_overlay_next", vec!["Right".to_string()]);
    m.insert("backtrack_overlay_confirm", vec!["Enter".to_string()]);

    m.insert("rui_cancel", vec!["Esc".to_string()]);
    m.insert(
        "rui_next_question",
        vec!["Ctrl+N".to_string(), "l".to_string()],
    );
    m.insert(
        "rui_prev_question",
        vec!["Ctrl+P".to_string(), "h".to_string()],
    );
    m.insert("rui_option_up", vec!["Up".to_string(), "k".to_string()]);
    m.insert("rui_option_down", vec!["Down".to_string(), "j".to_string()]);
    m.insert("rui_option_select", vec!["Space".to_string()]);
    m.insert("rui_option_clear", vec!["Backspace".to_string()]);
    m.insert("rui_option_to_notes", vec!["Tab".to_string()]);
    m.insert("rui_submit_or_next", vec!["Enter".to_string()]);
    m.insert("rui_notes_to_options", vec!["Tab".to_string()]);
    m.insert("rui_notes_backspace_empty", vec!["Backspace".to_string()]);

    m.insert(
        "list_up",
        vec!["Up".to_string(), "Ctrl+P".to_string(), "k".to_string()],
    );
    m.insert(
        "list_down",
        vec!["Down".to_string(), "Ctrl+N".to_string(), "j".to_string()],
    );
    m.insert("list_search_backspace", vec!["Backspace".to_string()]);
    m.insert("list_cancel", vec!["Esc".to_string()]);
    m.insert("list_accept", vec!["Enter".to_string()]);
    m.insert(
        "list_pick_index",
        vec![
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
            "5".to_string(),
            "6".to_string(),
            "7".to_string(),
            "8".to_string(),
            "9".to_string(),
        ],
    );

    m.insert("approval_approve", vec!["y".to_string()]);
    m.insert("approval_approve_policy", vec!["p".to_string()]);
    m.insert("approval_approve_session", vec!["a".to_string()]);
    m.insert("approval_reject", vec!["Esc".to_string(), "n".to_string()]);
    m.insert("approval_cancel", vec!["c".to_string()]);

    m.insert("skills_up", vec!["Up".to_string(), "Ctrl+P".to_string()]);
    m.insert(
        "skills_down",
        vec!["Down".to_string(), "Ctrl+N".to_string()],
    );
    m.insert(
        "skills_toggle",
        vec!["Space".to_string(), "Enter".to_string()],
    );
    m.insert("skills_search_backspace", vec!["Backspace".to_string()]);
    m.insert("skills_cancel", vec!["Esc".to_string()]);

    m.insert(
        "features_up",
        vec!["Up".to_string(), "Ctrl+P".to_string(), "k".to_string()],
    );
    m.insert(
        "features_down",
        vec!["Down".to_string(), "Ctrl+N".to_string(), "j".to_string()],
    );
    m.insert("features_toggle", vec!["Space".to_string()]);
    m.insert("features_cancel", vec!["Enter".to_string()]);

    m.insert("resume_exit", vec!["Ctrl+C".to_string()]);
    m.insert("resume_start_fresh", vec!["Esc".to_string()]);
    m.insert("resume_accept", vec!["Enter".to_string()]);
    m.insert("resume_up", vec!["Up".to_string()]);
    m.insert("resume_down", vec!["Down".to_string()]);
    m.insert("resume_page_up", vec!["PageUp".to_string()]);
    m.insert("resume_page_down", vec!["PageDown".to_string()]);
    m.insert("resume_search_backspace", vec!["Backspace".to_string()]);

    m.insert(
        "update_exit",
        vec!["Ctrl+C".to_string(), "Ctrl+D".to_string()],
    );
    m.insert("update_up", vec!["Up".to_string(), "k".to_string()]);
    m.insert("update_down", vec!["Down".to_string(), "j".to_string()]);
    m.insert("update_select_1", vec!["1".to_string()]);
    m.insert("update_select_2", vec!["2".to_string()]);
    m.insert("update_select_3", vec!["3".to_string()]);
    m.insert("update_confirm", vec!["Enter".to_string()]);
    m.insert("update_cancel", vec!["Esc".to_string()]);

    m.insert(
        "migration_exit",
        vec!["Ctrl+C".to_string(), "Ctrl+D".to_string()],
    );
    m.insert("migration_up", vec!["Up".to_string(), "k".to_string()]);
    m.insert("migration_down", vec!["Down".to_string(), "j".to_string()]);
    m.insert("migration_select_1", vec!["1".to_string()]);
    m.insert("migration_select_2", vec!["2".to_string()]);
    m.insert(
        "migration_confirm",
        vec!["Enter".to_string(), "Esc".to_string()],
    );

    m.insert("oss_cancel", vec!["Ctrl+C".to_string()]);
    m.insert("oss_left", vec!["Left".to_string()]);
    m.insert("oss_right", vec!["Right".to_string()]);
    m.insert("oss_confirm", vec!["Enter".to_string()]);
    m.insert("oss_default", vec!["Esc".to_string()]);
    m.insert("oss_select_l", vec!["l".to_string()]);
    m.insert("oss_select_o", vec!["o".to_string()]);
    m.insert("oss_select_c", vec!["c".to_string()]);

    m.insert("cwd_exit", vec!["Ctrl+C".to_string(), "Ctrl+D".to_string()]);
    m.insert("cwd_up", vec!["Up".to_string(), "k".to_string()]);
    m.insert("cwd_down", vec!["Down".to_string(), "j".to_string()]);
    m.insert(
        "cwd_select_session",
        vec!["1".to_string(), "Esc".to_string()],
    );
    m.insert("cwd_select_current", vec!["2".to_string()]);
    m.insert("cwd_confirm", vec!["Enter".to_string()]);

    m.insert(
        "onboarding_exit",
        vec!["Ctrl+C".to_string(), "Ctrl+D".to_string()],
    );
    m.insert("onboarding_quit", vec!["q".to_string()]);
    m.insert("welcome_cycle_animation", vec!["Ctrl+.".to_string()]);

    m.insert("auth_up", vec!["Up".to_string(), "k".to_string()]);
    m.insert("auth_down", vec!["Down".to_string(), "j".to_string()]);
    m.insert("auth_select_1", vec!["1".to_string()]);
    m.insert("auth_select_2", vec!["2".to_string()]);
    m.insert("auth_select_3", vec!["3".to_string()]);
    m.insert("auth_confirm", vec!["Enter".to_string()]);
    m.insert("auth_back", vec!["Esc".to_string()]);

    m.insert("auth_api_key_submit", vec!["Enter".to_string()]);
    m.insert("auth_api_key_back", vec!["Esc".to_string()]);
    m.insert("auth_api_key_backspace", vec!["Backspace".to_string()]);

    m.insert("trust_up", vec!["Up".to_string(), "k".to_string()]);
    m.insert("trust_down", vec!["Down".to_string(), "j".to_string()]);
    m.insert("trust_select_trust", vec!["1".to_string(), "y".to_string()]);
    m.insert(
        "trust_select_dont_trust",
        vec!["2".to_string(), "n".to_string()],
    );
    m.insert("trust_confirm", vec!["Enter".to_string()]);

    m
}

fn validate_conflicts(map: &TuiKeymap) -> Result<(), KeymapError> {
    let mut ctx = ContextConflicts::new("global");
    ctx.add("global_suspend", &map.global_suspend);
    ctx.add("global_show_transcript", &map.global_show_transcript);
    ctx.add("global_external_editor", &map.global_external_editor);
    ctx.add("global_backtrack_prime", &map.global_backtrack_prime);
    ctx.add("global_backtrack_confirm", &map.global_backtrack_confirm);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("chat");
    ctx.add(
        "chat_quit_or_interrupt_primary",
        &map.chat_quit_or_interrupt_primary,
    );
    ctx.add(
        "chat_quit_or_interrupt_secondary",
        &map.chat_quit_or_interrupt_secondary,
    );
    ctx.add("chat_paste_image", &map.chat_paste_image);
    ctx.add(
        "chat_recall_queued_message",
        &map.chat_recall_queued_message,
    );
    ctx.add("chat_change_mode", &map.chat_change_mode);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("composer");
    ctx.add("composer_submit", &map.composer_submit);
    ctx.add("composer_queue", &map.composer_queue);
    ctx.add("composer_newline", &map.composer_newline);
    ctx.add("composer_toggle_shortcuts", &map.composer_toggle_shortcuts);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("popup");
    ctx.add("popup_up", &map.popup_up);
    ctx.add("popup_down", &map.popup_down);
    ctx.add("popup_accept", &map.popup_accept);
    ctx.add("popup_cancel", &map.popup_cancel);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("text");
    ctx.add("text_newline", &map.text_newline);
    ctx.add("text_backspace", &map.text_backspace);
    ctx.add("text_delete_forward", &map.text_delete_forward);
    ctx.add("text_delete_backward_word", &map.text_delete_backward_word);
    ctx.add("text_delete_forward_word", &map.text_delete_forward_word);
    ctx.add("text_kill_line_start", &map.text_kill_line_start);
    ctx.add("text_kill_line_end", &map.text_kill_line_end);
    ctx.add("text_yank", &map.text_yank);
    ctx.add("text_move_left", &map.text_move_left);
    ctx.add("text_move_right", &map.text_move_right);
    ctx.add("text_move_up", &map.text_move_up);
    ctx.add("text_move_down", &map.text_move_down);
    ctx.add("text_word_left", &map.text_word_left);
    ctx.add("text_word_right", &map.text_word_right);
    ctx.add("text_line_start", &map.text_line_start);
    ctx.add("text_line_end", &map.text_line_end);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("pager");
    ctx.add("pager_scroll_up", &map.pager_scroll_up);
    ctx.add("pager_scroll_down", &map.pager_scroll_down);
    ctx.add("pager_page_up", &map.pager_page_up);
    ctx.add("pager_page_down", &map.pager_page_down);
    ctx.add("pager_half_page_up", &map.pager_half_page_up);
    ctx.add("pager_half_page_down", &map.pager_half_page_down);
    ctx.add("pager_jump_top", &map.pager_jump_top);
    ctx.add("pager_jump_bottom", &map.pager_jump_bottom);
    ctx.add("pager_quit", &map.pager_quit);
    ctx.add("pager_backtrack_prev", &map.pager_backtrack_prev);
    ctx.add("pager_backtrack_next", &map.pager_backtrack_next);
    ctx.add("pager_backtrack_confirm", &map.pager_backtrack_confirm);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("backtrack_overlay");
    ctx.add("backtrack_overlay_prev", &map.backtrack_overlay_prev);
    ctx.add("backtrack_overlay_next", &map.backtrack_overlay_next);
    ctx.add("backtrack_overlay_confirm", &map.backtrack_overlay_confirm);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("request_user_input_options");
    ctx.add("rui_cancel", &map.rui_cancel);
    ctx.add("rui_next_question", &map.rui_next_question);
    ctx.add("rui_prev_question", &map.rui_prev_question);
    ctx.add("rui_option_up", &map.rui_option_up);
    ctx.add("rui_option_down", &map.rui_option_down);
    ctx.add("rui_option_select", &map.rui_option_select);
    ctx.add("rui_option_clear", &map.rui_option_clear);
    ctx.add("rui_option_to_notes", &map.rui_option_to_notes);
    ctx.add("rui_submit_or_next", &map.rui_submit_or_next);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("request_user_input_notes");
    ctx.add("rui_cancel", &map.rui_cancel);
    ctx.add("rui_next_question", &map.rui_next_question);
    ctx.add("rui_prev_question", &map.rui_prev_question);
    ctx.add("rui_option_up", &map.rui_option_up);
    ctx.add("rui_option_down", &map.rui_option_down);
    ctx.add("rui_notes_to_options", &map.rui_notes_to_options);
    ctx.add("rui_notes_backspace_empty", &map.rui_notes_backspace_empty);
    ctx.add("rui_submit_or_next", &map.rui_submit_or_next);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("list_selection");
    ctx.add("list_up", &map.list_up);
    ctx.add("list_down", &map.list_down);
    ctx.add("list_search_backspace", &map.list_search_backspace);
    ctx.add("list_cancel", &map.list_cancel);
    ctx.add("list_accept", &map.list_accept);
    ctx.add("list_pick_index", &map.list_pick_index);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("approval");
    ctx.add("approval_approve", &map.approval_approve);
    ctx.add("approval_approve_policy", &map.approval_approve_policy);
    ctx.add("approval_approve_session", &map.approval_approve_session);
    ctx.add("approval_reject", &map.approval_reject);
    ctx.add("approval_cancel", &map.approval_cancel);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("skills_toggle");
    ctx.add("skills_up", &map.skills_up);
    ctx.add("skills_down", &map.skills_down);
    ctx.add("skills_toggle", &map.skills_toggle);
    ctx.add("skills_search_backspace", &map.skills_search_backspace);
    ctx.add("skills_cancel", &map.skills_cancel);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("experimental_features");
    ctx.add("features_up", &map.features_up);
    ctx.add("features_down", &map.features_down);
    ctx.add("features_toggle", &map.features_toggle);
    ctx.add("features_cancel", &map.features_cancel);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("resume_picker");
    ctx.add("resume_exit", &map.resume_exit);
    ctx.add("resume_start_fresh", &map.resume_start_fresh);
    ctx.add("resume_accept", &map.resume_accept);
    ctx.add("resume_up", &map.resume_up);
    ctx.add("resume_down", &map.resume_down);
    ctx.add("resume_page_up", &map.resume_page_up);
    ctx.add("resume_page_down", &map.resume_page_down);
    ctx.add("resume_search_backspace", &map.resume_search_backspace);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("update_prompt");
    ctx.add("update_exit", &map.update_exit);
    ctx.add("update_up", &map.update_up);
    ctx.add("update_down", &map.update_down);
    ctx.add("update_select_1", &map.update_select_1);
    ctx.add("update_select_2", &map.update_select_2);
    ctx.add("update_select_3", &map.update_select_3);
    ctx.add("update_confirm", &map.update_confirm);
    ctx.add("update_cancel", &map.update_cancel);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("model_migration");
    ctx.add("migration_exit", &map.migration_exit);
    ctx.add("migration_up", &map.migration_up);
    ctx.add("migration_down", &map.migration_down);
    ctx.add("migration_select_1", &map.migration_select_1);
    ctx.add("migration_select_2", &map.migration_select_2);
    ctx.add("migration_confirm", &map.migration_confirm);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("oss_selection");
    ctx.add("oss_cancel", &map.oss_cancel);
    ctx.add("oss_left", &map.oss_left);
    ctx.add("oss_right", &map.oss_right);
    ctx.add("oss_confirm", &map.oss_confirm);
    ctx.add("oss_default", &map.oss_default);
    ctx.add("oss_select_l", &map.oss_select_l);
    ctx.add("oss_select_o", &map.oss_select_o);
    ctx.add("oss_select_c", &map.oss_select_c);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("cwd_prompt");
    ctx.add("cwd_exit", &map.cwd_exit);
    ctx.add("cwd_up", &map.cwd_up);
    ctx.add("cwd_down", &map.cwd_down);
    ctx.add("cwd_select_session", &map.cwd_select_session);
    ctx.add("cwd_select_current", &map.cwd_select_current);
    ctx.add("cwd_confirm", &map.cwd_confirm);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("onboarding");
    ctx.add("onboarding_exit", &map.onboarding_exit);
    ctx.add("onboarding_quit", &map.onboarding_quit);
    ctx.add("welcome_cycle_animation", &map.welcome_cycle_animation);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("auth");
    ctx.add("auth_up", &map.auth_up);
    ctx.add("auth_down", &map.auth_down);
    ctx.add("auth_select_1", &map.auth_select_1);
    ctx.add("auth_select_2", &map.auth_select_2);
    ctx.add("auth_select_3", &map.auth_select_3);
    ctx.add("auth_confirm", &map.auth_confirm);
    ctx.add("auth_back", &map.auth_back);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("auth_api_key");
    ctx.add("auth_api_key_submit", &map.auth_api_key_submit);
    ctx.add("auth_api_key_back", &map.auth_api_key_back);
    ctx.add("auth_api_key_backspace", &map.auth_api_key_backspace);
    ctx.check()?;

    let mut ctx = ContextConflicts::new("trust_directory");
    ctx.add("trust_up", &map.trust_up);
    ctx.add("trust_down", &map.trust_down);
    ctx.add("trust_select_trust", &map.trust_select_trust);
    ctx.add("trust_select_dont_trust", &map.trust_select_dont_trust);
    ctx.add("trust_confirm", &map.trust_confirm);
    ctx.check()?;

    Ok(())
}

struct ContextConflicts {
    name: &'static str,
    used: HashMap<(KeyModifiers, KeyCode), Vec<String>>,
}

impl ContextConflicts {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            used: HashMap::new(),
        }
    }

    fn add(&mut self, action: &str, set: &KeyBindingSet) {
        for chord in &set.0 {
            for (mods, key) in chord.conflict_keys() {
                self.used
                    .entry((mods, key))
                    .or_default()
                    .push(action.to_string());
            }
        }
    }

    fn check(self) -> Result<(), KeymapError> {
        for ((mods, key), actions) in self.used {
            if actions.len() > 1 {
                let chord = format_keybinding(mods, key);
                return Err(KeymapError::Conflict {
                    context: self.name,
                    chord,
                    actions,
                });
            }
        }
        Ok(())
    }
}

pub const TUI_KEYBINDING_ACTIONS: &[&str] = &[
    "global_suspend",
    "global_show_transcript",
    "global_external_editor",
    "global_backtrack_prime",
    "global_backtrack_confirm",
    "chat_quit_or_interrupt_primary",
    "chat_quit_or_interrupt_secondary",
    "chat_paste_image",
    "chat_recall_queued_message",
    "chat_change_mode",
    "composer_submit",
    "composer_queue",
    "composer_newline",
    "composer_toggle_shortcuts",
    "popup_up",
    "popup_down",
    "popup_accept",
    "popup_cancel",
    "text_newline",
    "text_backspace",
    "text_delete_forward",
    "text_delete_backward_word",
    "text_delete_forward_word",
    "text_kill_line_start",
    "text_kill_line_end",
    "text_yank",
    "text_move_left",
    "text_move_right",
    "text_move_up",
    "text_move_down",
    "text_word_left",
    "text_word_right",
    "text_line_start",
    "text_line_end",
    "pager_scroll_up",
    "pager_scroll_down",
    "pager_page_up",
    "pager_page_down",
    "pager_half_page_up",
    "pager_half_page_down",
    "pager_jump_top",
    "pager_jump_bottom",
    "pager_quit",
    "pager_backtrack_prev",
    "pager_backtrack_next",
    "pager_backtrack_confirm",
    "backtrack_overlay_prev",
    "backtrack_overlay_next",
    "backtrack_overlay_confirm",
    "rui_cancel",
    "rui_next_question",
    "rui_prev_question",
    "rui_option_up",
    "rui_option_down",
    "rui_option_select",
    "rui_option_clear",
    "rui_option_to_notes",
    "rui_submit_or_next",
    "rui_notes_to_options",
    "rui_notes_backspace_empty",
    "list_up",
    "list_down",
    "list_search_backspace",
    "list_cancel",
    "list_accept",
    "list_pick_index",
    "approval_approve",
    "approval_approve_policy",
    "approval_approve_session",
    "approval_reject",
    "approval_cancel",
    "skills_up",
    "skills_down",
    "skills_toggle",
    "skills_search_backspace",
    "skills_cancel",
    "features_up",
    "features_down",
    "features_toggle",
    "features_cancel",
    "resume_exit",
    "resume_start_fresh",
    "resume_accept",
    "resume_up",
    "resume_down",
    "resume_page_up",
    "resume_page_down",
    "resume_search_backspace",
    "update_exit",
    "update_up",
    "update_down",
    "update_select_1",
    "update_select_2",
    "update_select_3",
    "update_confirm",
    "update_cancel",
    "migration_exit",
    "migration_up",
    "migration_down",
    "migration_select_1",
    "migration_select_2",
    "migration_confirm",
    "oss_cancel",
    "oss_left",
    "oss_right",
    "oss_confirm",
    "oss_default",
    "oss_select_l",
    "oss_select_o",
    "oss_select_c",
    "cwd_exit",
    "cwd_up",
    "cwd_down",
    "cwd_select_session",
    "cwd_select_current",
    "cwd_confirm",
    "onboarding_exit",
    "onboarding_quit",
    "welcome_cycle_animation",
    "auth_up",
    "auth_down",
    "auth_select_1",
    "auth_select_2",
    "auth_select_3",
    "auth_confirm",
    "auth_back",
    "auth_api_key_submit",
    "auth_api_key_back",
    "auth_api_key_backspace",
    "trust_up",
    "trust_down",
    "trust_select_trust",
    "trust_select_dont_trust",
    "trust_confirm",
];
