use ratatui::text::Line;

use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::theme;

pub(crate) fn build_theme_picker_params(
    current_name: Option<&str>,
    has_overrides: bool,
) -> SelectionViewParams {
    let original_theme = theme::current();
    let items = theme::builtin_theme_names()
        .iter()
        .map(|name| {
            let name = name.to_owned();
            SelectionItem {
                name: name.to_string(),
                // TODO: description: name.to_string(),
                is_current: current_name == Some(name),
                dismiss_on_select: true,
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::ThemeSelected {
                        name: name.to_string(),
                    });
                })],
                ..Default::default()
            }
        })
        .collect::<Vec<_>>();

    let footer_note: Option<Line<'static>> = if has_overrides {
        // write the footer note in red to warn the user that they have unsaved overrides that will be lost if they switch themes
        Some(
            Line::from("Note: selecting a theme will replace your custom palette/style overrides.")
                .style(theme::error()),
        )
    } else {
        None
    };

    SelectionViewParams {
        title: Some("Select Theme".to_string()),
        subtitle: Some("Use the arrow keys to navigate and Enter to select a theme.".to_string()),
        footer_note,
        items,
        on_selection_changed: Some(Box::new(move |idx, _tx| {
            let names = theme::builtin_theme_names();
            if let Some(name) = names.get(idx)
                && let Some(t) = theme::builtin_theme(name)
            {
                theme::set_theme(t);
            }
        })),
        on_cancel: Some(Box::new(move |_tx| {
            theme::set_theme(original_theme.clone());
        })),
        ..Default::default()
    }
}
