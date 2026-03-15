use std::path::Path;

use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::to_response;
use codex_app_server_protocol::CommandExecutionStatus;
use codex_app_server_protocol::ItemCompletedNotification;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::LocalShellStartParams;
use codex_app_server_protocol::LocalShellStartResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStartedNotification;
use codex_app_server_protocol::UserInput;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn local_shell_start_emits_thread_events_and_records_shell_output() -> Result<()> {
    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_assistant_message("msg-1", "done"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), &server.uri())?;

    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_request_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(thread_response)?;

    #[cfg(windows)]
    let command = "Write-Output shell-from-remote";
    #[cfg(not(windows))]
    let command = "printf shell-from-remote";

    let local_shell_request_id = mcp
        .send_local_shell_start_request(LocalShellStartParams {
            thread_id: thread.id.clone(),
            command: command.to_string(),
        })
        .await?;
    let local_shell_response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(local_shell_request_id)),
    )
    .await??;
    let LocalShellStartResponse { turn_id } = to_response(local_shell_response)?;

    let mut saw_command_started = false;
    let mut saw_command_completed = false;
    let mut saw_turn_started = false;

    loop {
        let message = timeout(DEFAULT_READ_TIMEOUT, mcp.read_next_message()).await??;
        let JSONRPCMessage::Notification(notification) = message else {
            continue;
        };
        match notification.method.as_str() {
            "turn/started" => {
                let started: TurnStartedNotification =
                    serde_json::from_value(notification.params.expect("turn/started params"))?;
                assert_eq!(started.turn.id, turn_id);
                saw_turn_started = true;
            }
            "item/started" => {
                let started: ItemStartedNotification =
                    serde_json::from_value(notification.params.expect("item/started params"))?;
                if started.turn_id != turn_id {
                    continue;
                }
                match started.item {
                    ThreadItem::CommandExecution { status, .. } => {
                        assert_eq!(status, CommandExecutionStatus::InProgress);
                        saw_command_started = true;
                    }
                    other => panic!("expected command execution item, got {other:?}"),
                }
            }
            "item/completed" => {
                let completed: ItemCompletedNotification =
                    serde_json::from_value(notification.params.expect("item/completed params"))?;
                if completed.turn_id != turn_id {
                    continue;
                }
                match completed.item {
                    ThreadItem::CommandExecution {
                        status,
                        aggregated_output,
                        ..
                    } => {
                        assert_eq!(status, CommandExecutionStatus::Completed);
                        let output = aggregated_output.expect("shell output should be captured");
                        assert!(
                            output.contains("shell-from-remote"),
                            "expected shell output in {output:?}"
                        );
                        saw_command_completed = true;
                    }
                    other => panic!("expected command execution item, got {other:?}"),
                }
            }
            "turn/completed" => {
                let completed: TurnCompletedNotification =
                    serde_json::from_value(notification.params.expect("turn/completed params"))?;
                if completed.turn.id == turn_id {
                    break;
                }
            }
            _ => {}
        }
    }

    assert!(saw_turn_started, "expected turn/started notification");
    assert!(saw_command_started, "expected item/started notification");
    assert!(
        saw_command_completed,
        "expected item/completed notification"
    );

    let follow_up_request_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id,
            input: vec![UserInput::Text {
                text: "follow-up after shell command".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let follow_up_response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(follow_up_request_id)),
    )
    .await??;
    let _: TurnStartResponse = to_response(follow_up_response)?;

    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let request = response_mock.single_request();
    let shell_history_entry = request
        .message_input_texts("user")
        .into_iter()
        .find(|text| text.contains("<user_shell_command>"))
        .expect("shell command should be injected into the next model request")
        .replace("\r\n", "\n");
    assert!(shell_history_entry.contains(command));
    assert!(shell_history_entry.contains("shell-from-remote"));

    Ok(())
}

fn create_config_toml(codex_home: &Path, server_uri: &str) -> std::io::Result<()> {
    let config_toml = codex_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}
