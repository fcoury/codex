use crate::config::ConfigToml;
use crate::config::types::RawMcpServerConfig;
use crate::features::FEATURES;
use schemars::r#gen::SchemaGenerator;
use schemars::r#gen::SchemaSettings;
use schemars::schema::ArrayValidation;
use schemars::schema::InstanceType;
use schemars::schema::ObjectValidation;
use schemars::schema::RootSchema;
use schemars::schema::Schema;
use schemars::schema::SchemaObject;
use schemars::schema::SingleOrVec;
use schemars::schema::SubschemaValidation;
use serde_json::Map;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Schema for the `[features]` map with known + legacy keys only.
pub(crate) fn features_schema(schema_gen: &mut SchemaGenerator) -> Schema {
    let mut object = SchemaObject {
        instance_type: Some(InstanceType::Object.into()),
        ..Default::default()
    };

    let mut validation = ObjectValidation::default();
    for feature in FEATURES {
        validation
            .properties
            .insert(feature.key.to_string(), schema_gen.subschema_for::<bool>());
    }
    for legacy_key in crate::features::legacy_feature_keys() {
        validation
            .properties
            .insert(legacy_key.to_string(), schema_gen.subschema_for::<bool>());
    }
    validation.additional_properties = Some(Box::new(Schema::Bool(false)));
    object.object = Some(Box::new(validation));

    Schema::Object(object)
}

/// Schema for the `[mcp_servers]` map using the raw input shape.
pub(crate) fn mcp_servers_schema(schema_gen: &mut SchemaGenerator) -> Schema {
    let mut object = SchemaObject {
        instance_type: Some(InstanceType::Object.into()),
        ..Default::default()
    };

    let validation = ObjectValidation {
        additional_properties: Some(Box::new(schema_gen.subschema_for::<RawMcpServerConfig>())),
        ..Default::default()
    };
    object.object = Some(Box::new(validation));

    Schema::Object(object)
}

/// Build the config schema for `config.toml`.
pub fn config_schema() -> RootSchema {
    SchemaSettings::draft07()
        .with(|settings| {
            settings.option_add_null_type = false;
        })
        .into_generator()
        .into_root_schema_for::<ConfigToml>()
}

/// Canonicalize a JSON value by sorting its keys.
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let mut sorted = Map::with_capacity(map.len());
            for (key, child) in entries {
                sorted.insert(key.clone(), canonicalize(child));
            }
            Value::Object(sorted)
        }
        _ => value.clone(),
    }
}

/// Render the config schema as pretty-printed JSON.
pub fn config_schema_json() -> anyhow::Result<Vec<u8>> {
    let schema = config_schema();
    let value = serde_json::to_value(schema)?;
    let value = canonicalize(&value);
    let json = serde_json::to_vec_pretty(&value)?;
    Ok(json)
}

/// Write the config schema fixture to disk.
pub fn write_config_schema(out_path: &Path) -> anyhow::Result<()> {
    let json = config_schema_json()?;
    std::fs::write(out_path, json)?;
    Ok(())
}

pub fn tui_keybindings_schema(_: &mut SchemaGenerator) -> Schema {
    // string schema
    let mut string_obj = SchemaObject::default();
    string_obj.instance_type = Some(SingleOrVec::Single(Box::new(InstanceType::String)));

    // array<string> schema
    let mut array_obj = SchemaObject::default();
    array_obj.instance_type = Some(SingleOrVec::Single(Box::new(InstanceType::Array)));
    array_obj.array = Some(Box::new(ArrayValidation {
        items: Some(SingleOrVec::Single(Box::new(Schema::Object(
            string_obj.clone(),
        )))),
        ..Default::default()
    }));

    // value is oneOf [string, array<string>]
    let mut value_obj = SchemaObject::default();
    value_obj.subschemas = Some(Box::new(SubschemaValidation {
        one_of: Some(vec![Schema::Object(string_obj), Schema::Object(array_obj)]),
        ..Default::default()
    }));

    // Build explicit properties
    let mut props = BTreeMap::new();
    for action in TUI_KEYBINDING_ACTIONS {
        props.insert((*action).to_string(), Schema::Object(value_obj.clone()));
    }

    let mut root = SchemaObject::default();
    root.instance_type = Some(SingleOrVec::Single(Box::new(InstanceType::Object)));
    root.object = Some(Box::new(ObjectValidation {
        properties: props,
        additional_properties: Some(Box::new(Schema::Bool(false))),
        ..Default::default()
    }));

    Schema::Object(root)
}

// List of all action IDs
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

#[cfg(test)]
mod tests {
    use super::canonicalize;
    use super::config_schema_json;

    use similar::TextDiff;

    #[test]
    fn config_schema_matches_fixture() {
        let fixture_path = codex_utils_cargo_bin::find_resource!("config.schema.json")
            .expect("resolve config schema fixture path");
        let fixture = std::fs::read_to_string(fixture_path).expect("read config schema fixture");
        let fixture_value: serde_json::Value =
            serde_json::from_str(&fixture).expect("parse config schema fixture");
        let schema_json = config_schema_json().expect("serialize config schema");
        let schema_value: serde_json::Value =
            serde_json::from_slice(&schema_json).expect("decode schema json");
        let fixture_value = canonicalize(&fixture_value);
        let schema_value = canonicalize(&schema_value);
        if fixture_value != schema_value {
            let expected =
                serde_json::to_string_pretty(&fixture_value).expect("serialize fixture json");
            let actual =
                serde_json::to_string_pretty(&schema_value).expect("serialize schema json");
            let diff = TextDiff::from_lines(&expected, &actual)
                .unified_diff()
                .header("fixture", "generated")
                .to_string();
            panic!(
                "Current schema for `config.toml` doesn't match the fixture. \
Run `just write-config-schema` to overwrite with your changes.\n\n{diff}"
            );
        }
    }
}
