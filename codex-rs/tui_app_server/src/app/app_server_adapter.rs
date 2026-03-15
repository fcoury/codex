/*
This module holds the temporary adapter layer between the TUI and the app
server during the hybrid migration period.

For now, the TUI still owns its existing direct-core behavior, but startup
allocates a local in-process app server and drains its event stream. Keeping
the app-server-specific wiring here keeps that transitional logic out of the
main `app.rs` orchestration path.

As more TUI flows move onto the app-server surface directly, this adapter
should shrink and eventually disappear.
*/

use super::App;
use crate::app_event::AppEvent;
use crate::app_server_session::AppServerSession;
use crate::app_server_session::app_server_rate_limit_snapshot_to_core;
use crate::app_server_session::status_account_display_from_auth_mode;
use codex_app_server_client::AppServerEvent;
use codex_app_server_protocol::CommandExecutionApprovalDecision;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::RequestId as AppServerRequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::Thread;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::TurnStatus;
use codex_protocol::ThreadId;
use codex_protocol::approvals::ElicitationRequestEvent;
use codex_protocol::approvals::ExecApprovalRequestEvent;
use codex_protocol::config_types::ModeKind;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::AgentMessageItem;
use codex_protocol::items::ContextCompactionItem;
use codex_protocol::items::ImageGenerationItem;
use codex_protocol::items::PlanItem;
use codex_protocol::items::ReasoningItem;
use codex_protocol::items::TurnItem;
use codex_protocol::items::UserMessageItem;
use codex_protocol::items::WebSearchItem;
use codex_protocol::mcp::RequestId as McpRequestId;
use codex_protocol::protocol::AgentMessageDeltaEvent;
use codex_protocol::protocol::AgentReasoningDeltaEvent;
use codex_protocol::protocol::AgentReasoningRawContentDeltaEvent;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::ItemStartedEvent;
use codex_protocol::protocol::PlanDeltaEvent;
use codex_protocol::protocol::RealtimeConversationClosedEvent;
use codex_protocol::protocol::RealtimeConversationRealtimeEvent;
use codex_protocol::protocol::RealtimeConversationStartedEvent;
use codex_protocol::protocol::RealtimeEvent;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::ThreadNameUpdatedEvent;
use codex_protocol::protocol::TokenCountEvent;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TokenUsageInfo;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::protocol::TurnAbortedEvent;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::request_permissions::RequestPermissionProfile;
use codex_protocol::request_permissions::RequestPermissionsEvent;
use codex_protocol::request_user_input::RequestUserInputEvent;
use codex_protocol::request_user_input::RequestUserInputQuestion;
use codex_protocol::request_user_input::RequestUserInputQuestionOption;
use serde_json::Value;

impl App {
    pub(super) async fn handle_app_server_event(
        &mut self,
        app_server_client: &AppServerSession,
        event: AppServerEvent,
    ) {
        match event {
            AppServerEvent::Lagged { skipped } => {
                tracing::warn!(
                    skipped,
                    "app-server event consumer lagged; dropping ignored events"
                );
            }
            AppServerEvent::ServerNotification(notification) => match notification {
                ServerNotification::ServerRequestResolved(notification) => {
                    self.pending_app_server_requests
                        .resolve_notification(&notification.request_id);
                }
                ServerNotification::CommandExecOutputDelta(notification) => {
                    if let Some((thread_id, event)) =
                        self.note_command_exec_output_delta(&notification)
                        && let Err(err) = self.enqueue_thread_event(thread_id, event).await
                    {
                        tracing::warn!(
                            "failed to enqueue app-server command exec output for {thread_id}: {err}"
                        );
                    }
                }
                ServerNotification::AccountRateLimitsUpdated(notification) => {
                    self.chat_widget.on_rate_limit_snapshot(Some(
                        app_server_rate_limit_snapshot_to_core(notification.rate_limits),
                    ));
                }
                ServerNotification::AccountUpdated(notification) => {
                    self.chat_widget.update_account_state(
                        status_account_display_from_auth_mode(
                            notification.auth_mode,
                            notification.plan_type,
                        ),
                        notification.plan_type,
                        matches!(
                            notification.auth_mode,
                            Some(codex_app_server_protocol::AuthMode::Chatgpt)
                        ),
                    );
                }
                notification => {
                    if let Some((thread_id, events)) =
                        server_notification_thread_events(notification)
                    {
                        for event in events {
                            self.enqueue_app_server_thread_event(
                                thread_id,
                                event,
                                "app-server server notification",
                            )
                            .await;
                        }
                    }
                }
            },
            AppServerEvent::LegacyNotification(notification) => {
                if let Some((thread_id, event)) = legacy_thread_event(notification.params) {
                    self.pending_app_server_requests.note_legacy_event(&event);
                    self.enqueue_app_server_thread_event(thread_id, event, "app-server event")
                        .await;
                }
            }
            AppServerEvent::ServerRequest(request) => {
                if let Some(unsupported) = self
                    .pending_app_server_requests
                    .note_server_request(&request)
                {
                    tracing::warn!(
                        request_id = ?unsupported.request_id,
                        message = unsupported.message,
                        "rejecting unsupported app-server request"
                    );
                    self.chat_widget
                        .add_error_message(unsupported.message.clone());
                    if let Err(err) = self
                        .reject_app_server_request(
                            app_server_client,
                            unsupported.request_id,
                            unsupported.message,
                        )
                        .await
                    {
                        tracing::warn!("{err}");
                    }
                } else if app_server_client.is_remote() {
                    match server_request_thread_event(&request) {
                        Ok((thread_id, event)) => {
                            self.pending_app_server_requests.note_legacy_event(&event);
                            self.enqueue_app_server_thread_event(
                                thread_id,
                                event,
                                "app-server remote server request",
                            )
                            .await;
                        }
                        Err(message) => {
                            tracing::warn!(
                                request_id = ?request.id(),
                                "{message}"
                            );
                            self.chat_widget.add_error_message(message.clone());
                            if let Err(err) = self
                                .reject_app_server_request(
                                    app_server_client,
                                    request.id().clone(),
                                    message,
                                )
                                .await
                            {
                                tracing::warn!("{err}");
                            }
                        }
                    }
                }
            }
            AppServerEvent::Disconnected { message } => {
                tracing::warn!("app-server event stream disconnected: {message}");
                self.chat_widget.add_error_message(message.clone());
                self.app_event_tx.send(AppEvent::FatalExitRequest(message));
            }
        }
    }

    async fn reject_app_server_request(
        &self,
        app_server_client: &AppServerSession,
        request_id: codex_app_server_protocol::RequestId,
        reason: String,
    ) -> std::result::Result<(), String> {
        app_server_client
            .reject_server_request(
                request_id,
                JSONRPCErrorError {
                    code: -32000,
                    message: reason,
                    data: None,
                },
            )
            .await
            .map_err(|err| format!("failed to reject app-server request: {err}"))
    }

    async fn enqueue_app_server_thread_event(
        &mut self,
        thread_id: ThreadId,
        event: Event,
        context: &str,
    ) {
        if self.primary_thread_id.is_none()
            || matches!(event.msg, EventMsg::SessionConfigured(_))
                && self.primary_thread_id == Some(thread_id)
        {
            if let Err(err) = self.enqueue_primary_event(event).await {
                tracing::warn!("failed to enqueue primary {context}: {err}");
            }
        } else if let Err(err) = self.enqueue_thread_event(thread_id, event).await {
            tracing::warn!("failed to enqueue {context} for {thread_id}: {err}");
        }
    }
}

pub(super) fn thread_snapshot_events(thread: &Thread) -> Vec<Event> {
    let Ok(thread_id) = ThreadId::from_string(&thread.id) else {
        tracing::warn!(
            thread_id = %thread.id,
            "ignoring app-server thread snapshot with invalid thread id"
        );
        return Vec::new();
    };

    thread
        .turns
        .iter()
        .flat_map(|turn| turn_snapshot_events(thread_id, turn))
        .collect()
}

fn server_request_thread_event(request: &ServerRequest) -> Result<(ThreadId, Event), String> {
    match request {
        ServerRequest::CommandExecutionRequestApproval { params, .. } => Ok((
            thread_id_from_remote_request("command execution approval", &params.thread_id)?,
            Event {
                id: String::new(),
                msg: EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
                    call_id: params.item_id.clone(),
                    approval_id: params.approval_id.clone(),
                    turn_id: params.turn_id.clone(),
                    command: params
                        .command
                        .as_deref()
                        .map(split_remote_command_for_approval)
                        .unwrap_or_default(),
                    cwd: params.cwd.clone().unwrap_or_default(),
                    reason: params.reason.clone(),
                    network_approval_context: params.network_approval_context.clone().map(
                        |context| codex_protocol::protocol::NetworkApprovalContext {
                            host: context.host,
                            protocol: context.protocol.to_core(),
                        },
                    ),
                    proposed_execpolicy_amendment: params
                        .proposed_execpolicy_amendment
                        .clone()
                        .map(|amendment| amendment.into_core()),
                    proposed_network_policy_amendments: params
                        .proposed_network_policy_amendments
                        .clone()
                        .map(|amendments| {
                            amendments
                                .into_iter()
                                .map(|amendment| amendment.into_core())
                                .collect()
                        }),
                    additional_permissions: params.additional_permissions.clone().map(Into::into),
                    skill_metadata: None,
                    available_decisions: params.available_decisions.clone().map(|decisions| {
                        decisions
                            .into_iter()
                            .map(command_approval_decision_to_review_decision)
                            .collect()
                    }),
                    parsed_cmd: params
                        .command_actions
                        .clone()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|action| action.into_core())
                        .collect(),
                }),
            },
        )),
        ServerRequest::PermissionsRequestApproval { params, .. } => Ok((
            thread_id_from_remote_request("permissions approval", &params.thread_id)?,
            Event {
                id: String::new(),
                msg: EventMsg::RequestPermissions(RequestPermissionsEvent {
                    call_id: params.item_id.clone(),
                    turn_id: params.turn_id.clone(),
                    reason: params.reason.clone(),
                    permissions: RequestPermissionProfile::from(Into::<
                        codex_protocol::models::PermissionProfile,
                    >::into(
                        params.permissions.clone()
                    )),
                }),
            },
        )),
        ServerRequest::ToolRequestUserInput { params, .. } => Ok((
            thread_id_from_remote_request("request_user_input", &params.thread_id)?,
            Event {
                id: String::new(),
                msg: EventMsg::RequestUserInput(RequestUserInputEvent {
                    call_id: params.item_id.clone(),
                    turn_id: params.turn_id.clone(),
                    questions: params
                        .questions
                        .iter()
                        .map(|question| RequestUserInputQuestion {
                            id: question.id.clone(),
                            header: question.header.clone(),
                            question: question.question.clone(),
                            is_other: question.is_other,
                            is_secret: question.is_secret,
                            options: question.options.as_ref().map(|options| {
                                options
                                    .iter()
                                    .map(|option| RequestUserInputQuestionOption {
                                        label: option.label.clone(),
                                        description: option.description.clone(),
                                    })
                                    .collect()
                            }),
                        })
                        .collect(),
                }),
            },
        )),
        ServerRequest::McpServerElicitationRequest { request_id, params } => Ok((
            thread_id_from_remote_request("MCP elicitation request", &params.thread_id)?,
            Event {
                id: String::new(),
                msg: EventMsg::ElicitationRequest(ElicitationRequestEvent {
                    turn_id: params.turn_id.clone(),
                    server_name: params.server_name.clone(),
                    id: app_server_request_id_to_mcp_request_id(request_id),
                    request: serde_json::from_value(
                        serde_json::to_value(&params.request).map_err(|err| {
                            format!(
                                "failed to encode remote MCP elicitation request for `{}`: {err}",
                                params.server_name
                            )
                        })?,
                    )
                    .map_err(|err| {
                        format!(
                            "failed to decode remote MCP elicitation request for `{}`: {err}",
                            params.server_name
                        )
                    })?,
                }),
            },
        )),
        ServerRequest::FileChangeRequestApproval { .. } => {
            Err("Remote file change approvals are not available in app-server TUI yet.".to_string())
        }
        ServerRequest::DynamicToolCall { .. }
        | ServerRequest::ChatgptAuthTokensRefresh { .. }
        | ServerRequest::ApplyPatchApproval { .. }
        | ServerRequest::ExecCommandApproval { .. } => {
            Err("This app-server request is not available in app-server TUI yet.".to_string())
        }
    }
}

fn thread_id_from_remote_request(context: &str, thread_id: &str) -> Result<ThreadId, String> {
    ThreadId::from_string(thread_id)
        .map_err(|err| format!("failed to parse remote {context} thread id `{thread_id}`: {err}"))
}

fn split_remote_command_for_approval(command: &str) -> Vec<String> {
    shlex::split(command).unwrap_or_else(|| vec![command.to_string()])
}

fn command_approval_decision_to_review_decision(
    decision: CommandExecutionApprovalDecision,
) -> ReviewDecision {
    match decision {
        CommandExecutionApprovalDecision::Accept => ReviewDecision::Approved,
        CommandExecutionApprovalDecision::AcceptForSession => ReviewDecision::ApprovedForSession,
        CommandExecutionApprovalDecision::AcceptWithExecpolicyAmendment {
            execpolicy_amendment,
        } => ReviewDecision::ApprovedExecpolicyAmendment {
            proposed_execpolicy_amendment: execpolicy_amendment.into_core(),
        },
        CommandExecutionApprovalDecision::ApplyNetworkPolicyAmendment {
            network_policy_amendment,
        } => ReviewDecision::NetworkPolicyAmendment {
            network_policy_amendment: network_policy_amendment.into_core(),
        },
        CommandExecutionApprovalDecision::Decline => ReviewDecision::Denied,
        CommandExecutionApprovalDecision::Cancel => ReviewDecision::Abort,
    }
}

fn app_server_request_id_to_mcp_request_id(request_id: &AppServerRequestId) -> McpRequestId {
    match request_id {
        AppServerRequestId::String(value) => McpRequestId::String(value.clone()),
        AppServerRequestId::Integer(value) => McpRequestId::Integer(*value),
    }
}

fn legacy_thread_event(params: Option<Value>) -> Option<(ThreadId, Event)> {
    let Value::Object(mut params) = params? else {
        return None;
    };
    let thread_id = params
        .remove("conversationId")
        .and_then(|value| serde_json::from_value::<String>(value).ok())
        .and_then(|value| ThreadId::from_string(&value).ok());
    let event = serde_json::from_value::<Event>(Value::Object(params)).ok()?;
    let thread_id = thread_id.or(match &event.msg {
        EventMsg::SessionConfigured(session) => Some(session.session_id),
        _ => None,
    })?;
    Some((thread_id, event))
}

fn server_notification_thread_events(
    notification: ServerNotification,
) -> Option<(ThreadId, Vec<Event>)> {
    match notification {
        ServerNotification::ThreadTokenUsageUpdated(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::TokenCount(TokenCountEvent {
                    info: Some(TokenUsageInfo {
                        total_token_usage: token_usage_from_app_server(
                            notification.token_usage.total,
                        ),
                        last_token_usage: token_usage_from_app_server(
                            notification.token_usage.last,
                        ),
                        model_context_window: notification.token_usage.model_context_window,
                    }),
                    rate_limits: None,
                }),
            }],
        )),
        ServerNotification::Error(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::Error(ErrorEvent {
                    message: notification.error.message,
                    codex_error_info: notification
                        .error
                        .codex_error_info
                        .and_then(app_server_codex_error_info_to_core),
                }),
            }],
        )),
        ServerNotification::ThreadNameUpdated(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::ThreadNameUpdated(ThreadNameUpdatedEvent {
                    thread_id: ThreadId::from_string(&notification.thread_id).ok()?,
                    thread_name: notification.thread_name,
                }),
            }],
        )),
        ServerNotification::TurnStarted(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::TurnStarted(TurnStartedEvent {
                    turn_id: notification.turn.id,
                    model_context_window: None,
                    collaboration_mode_kind: ModeKind::default(),
                }),
            }],
        )),
        ServerNotification::TurnCompleted(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::TurnComplete(TurnCompleteEvent {
                    turn_id: notification.turn.id,
                    last_agent_message: None,
                }),
            }],
        )),
        ServerNotification::ItemStarted(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::ItemStarted(ItemStartedEvent {
                    thread_id: ThreadId::from_string(&notification.thread_id).ok()?,
                    turn_id: notification.turn_id,
                    item: thread_item_to_core(notification.item)?,
                }),
            }],
        )),
        ServerNotification::ItemCompleted(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::ItemCompleted(ItemCompletedEvent {
                    thread_id: ThreadId::from_string(&notification.thread_id).ok()?,
                    turn_id: notification.turn_id,
                    item: thread_item_to_core(notification.item)?,
                }),
            }],
        )),
        ServerNotification::AgentMessageDelta(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::AgentMessageDelta(AgentMessageDeltaEvent {
                    delta: notification.delta,
                }),
            }],
        )),
        ServerNotification::PlanDelta(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::PlanDelta(PlanDeltaEvent {
                    thread_id: notification.thread_id,
                    turn_id: notification.turn_id,
                    item_id: notification.item_id,
                    delta: notification.delta,
                }),
            }],
        )),
        ServerNotification::ReasoningSummaryTextDelta(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::AgentReasoningDelta(AgentReasoningDeltaEvent {
                    delta: notification.delta,
                }),
            }],
        )),
        ServerNotification::ReasoningTextDelta(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::AgentReasoningRawContentDelta(AgentReasoningRawContentDeltaEvent {
                    delta: notification.delta,
                }),
            }],
        )),
        ServerNotification::ThreadRealtimeStarted(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::RealtimeConversationStarted(RealtimeConversationStartedEvent {
                    session_id: notification.session_id,
                }),
            }],
        )),
        ServerNotification::ThreadRealtimeItemAdded(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::RealtimeConversationRealtime(RealtimeConversationRealtimeEvent {
                    payload: RealtimeEvent::ConversationItemAdded(notification.item),
                }),
            }],
        )),
        ServerNotification::ThreadRealtimeOutputAudioDelta(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::RealtimeConversationRealtime(RealtimeConversationRealtimeEvent {
                    payload: RealtimeEvent::AudioOut(notification.audio.into()),
                }),
            }],
        )),
        ServerNotification::ThreadRealtimeError(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::RealtimeConversationRealtime(RealtimeConversationRealtimeEvent {
                    payload: RealtimeEvent::Error(notification.message),
                }),
            }],
        )),
        ServerNotification::ThreadRealtimeClosed(notification) => Some((
            ThreadId::from_string(&notification.thread_id).ok()?,
            vec![Event {
                id: String::new(),
                msg: EventMsg::RealtimeConversationClosed(RealtimeConversationClosedEvent {
                    reason: notification.reason,
                }),
            }],
        )),
        _ => None,
    }
}

fn token_usage_from_app_server(
    value: codex_app_server_protocol::TokenUsageBreakdown,
) -> TokenUsage {
    TokenUsage {
        input_tokens: value.input_tokens,
        cached_input_tokens: value.cached_input_tokens,
        output_tokens: value.output_tokens,
        reasoning_output_tokens: value.reasoning_output_tokens,
        total_tokens: value.total_tokens,
    }
}

fn turn_snapshot_events(thread_id: ThreadId, turn: &Turn) -> Vec<Event> {
    let mut events = vec![Event {
        id: String::new(),
        msg: EventMsg::TurnStarted(TurnStartedEvent {
            turn_id: turn.id.clone(),
            model_context_window: None,
            collaboration_mode_kind: ModeKind::default(),
        }),
    }];

    events.extend(turn.items.iter().filter_map(|item| {
        let item = thread_item_to_core(item.clone())?;
        Some(Event {
            id: String::new(),
            msg: EventMsg::ItemCompleted(ItemCompletedEvent {
                thread_id,
                turn_id: turn.id.clone(),
                item,
            }),
        })
    }));

    match turn.status {
        TurnStatus::Completed => events.push(Event {
            id: String::new(),
            msg: EventMsg::TurnComplete(TurnCompleteEvent {
                turn_id: turn.id.clone(),
                last_agent_message: None,
            }),
        }),
        TurnStatus::Interrupted => events.push(Event {
            id: String::new(),
            msg: EventMsg::TurnAborted(TurnAbortedEvent {
                turn_id: Some(turn.id.clone()),
                reason: TurnAbortReason::Interrupted,
            }),
        }),
        TurnStatus::Failed => {
            if let Some(error) = &turn.error {
                events.push(Event {
                    id: String::new(),
                    msg: EventMsg::Error(ErrorEvent {
                        message: error.message.clone(),
                        codex_error_info: error
                            .codex_error_info
                            .clone()
                            .and_then(app_server_codex_error_info_to_core),
                    }),
                });
            }
            events.push(Event {
                id: String::new(),
                msg: EventMsg::TurnComplete(TurnCompleteEvent {
                    turn_id: turn.id.clone(),
                    last_agent_message: None,
                }),
            });
        }
        TurnStatus::InProgress => {}
    }

    events
}

fn thread_item_to_core(item: ThreadItem) -> Option<TurnItem> {
    match item {
        ThreadItem::UserMessage { id, content } => Some(TurnItem::UserMessage(UserMessageItem {
            id,
            content: content
                .into_iter()
                .map(codex_app_server_protocol::UserInput::into_core)
                .collect(),
        })),
        ThreadItem::AgentMessage { id, text, phase } => {
            Some(TurnItem::AgentMessage(AgentMessageItem {
                id,
                content: vec![AgentMessageContent::Text { text }],
                phase,
            }))
        }
        ThreadItem::Plan { id, text } => Some(TurnItem::Plan(PlanItem { id, text })),
        ThreadItem::Reasoning {
            id,
            summary,
            content,
        } => Some(TurnItem::Reasoning(ReasoningItem {
            id,
            summary_text: summary,
            raw_content: content,
        })),
        ThreadItem::WebSearch { id, query, action } => Some(TurnItem::WebSearch(WebSearchItem {
            id,
            query,
            action: app_server_web_search_action_to_core(action?)?,
        })),
        ThreadItem::ImageGeneration {
            id,
            status,
            revised_prompt,
            result,
        } => Some(TurnItem::ImageGeneration(ImageGenerationItem {
            id,
            status,
            revised_prompt,
            result,
            saved_path: None,
        })),
        ThreadItem::ContextCompaction { id } => {
            Some(TurnItem::ContextCompaction(ContextCompactionItem { id }))
        }
        ThreadItem::CommandExecution { .. }
        | ThreadItem::FileChange { .. }
        | ThreadItem::McpToolCall { .. }
        | ThreadItem::DynamicToolCall { .. }
        | ThreadItem::CollabAgentToolCall { .. }
        | ThreadItem::ImageView { .. }
        | ThreadItem::EnteredReviewMode { .. }
        | ThreadItem::ExitedReviewMode { .. } => {
            tracing::debug!("ignoring unsupported app-server thread item in TUI adapter");
            None
        }
    }
}

fn app_server_web_search_action_to_core(
    action: codex_app_server_protocol::WebSearchAction,
) -> Option<codex_protocol::models::WebSearchAction> {
    match action {
        codex_app_server_protocol::WebSearchAction::Search { query, queries } => {
            Some(codex_protocol::models::WebSearchAction::Search { query, queries })
        }
        codex_app_server_protocol::WebSearchAction::OpenPage { url } => {
            Some(codex_protocol::models::WebSearchAction::OpenPage { url })
        }
        codex_app_server_protocol::WebSearchAction::FindInPage { url, pattern } => {
            Some(codex_protocol::models::WebSearchAction::FindInPage { url, pattern })
        }
        codex_app_server_protocol::WebSearchAction::Other => None,
    }
}

fn app_server_codex_error_info_to_core(
    value: codex_app_server_protocol::CodexErrorInfo,
) -> Option<codex_protocol::protocol::CodexErrorInfo> {
    serde_json::from_value(serde_json::to_value(value).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::server_notification_thread_events;
    use super::server_request_thread_event;
    use super::thread_snapshot_events;
    use codex_app_server_protocol::AgentMessageDeltaNotification;
    use codex_app_server_protocol::CommandExecutionApprovalDecision;
    use codex_app_server_protocol::CommandExecutionRequestApprovalParams;
    use codex_app_server_protocol::ItemCompletedNotification;
    use codex_app_server_protocol::PermissionsRequestApprovalParams;
    use codex_app_server_protocol::ReasoningSummaryTextDeltaNotification;
    use codex_app_server_protocol::RequestId as AppServerRequestId;
    use codex_app_server_protocol::ServerNotification;
    use codex_app_server_protocol::ServerRequest;
    use codex_app_server_protocol::Thread;
    use codex_app_server_protocol::ThreadItem;
    use codex_app_server_protocol::ThreadStatus;
    use codex_app_server_protocol::Turn;
    use codex_app_server_protocol::TurnCompletedNotification;
    use codex_app_server_protocol::TurnStatus;
    use codex_protocol::ThreadId;
    use codex_protocol::items::AgentMessageContent;
    use codex_protocol::items::AgentMessageItem;
    use codex_protocol::items::TurnItem;
    use codex_protocol::models::MessagePhase;
    use codex_protocol::protocol::EventMsg;
    use codex_protocol::protocol::NetworkPolicyRuleAction;
    use codex_protocol::protocol::ReviewDecision;
    use codex_protocol::protocol::SessionSource;
    use codex_protocol::protocol::TurnAbortReason;
    use codex_protocol::protocol::TurnAbortedEvent;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;

    #[test]
    fn bridges_completed_agent_messages_from_server_notifications() {
        let thread_id = "019cee8c-b993-7e33-88c0-014d4e62612d".to_string();
        let turn_id = "019cee8c-b9b4-7f10-a1b0-38caa876a012".to_string();
        let item_id = "msg_123".to_string();

        let (actual_thread_id, events) = server_notification_thread_events(
            ServerNotification::ItemCompleted(ItemCompletedNotification {
                item: ThreadItem::AgentMessage {
                    id: item_id,
                    text: "Hello from your coding assistant.".to_string(),
                    phase: Some(MessagePhase::FinalAnswer),
                },
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
            }),
        )
        .expect("notification should bridge");

        assert_eq!(
            actual_thread_id,
            ThreadId::from_string(&thread_id).expect("valid thread id")
        );
        let [event] = events.as_slice() else {
            panic!("expected one bridged event");
        };
        assert_eq!(event.id, String::new());
        let EventMsg::ItemCompleted(completed) = &event.msg else {
            panic!("expected item completed event");
        };
        assert_eq!(
            completed.thread_id,
            ThreadId::from_string(&thread_id).expect("valid thread id")
        );
        assert_eq!(completed.turn_id, turn_id);
        match &completed.item {
            TurnItem::AgentMessage(AgentMessageItem { id, content, phase }) => {
                assert_eq!(id, "msg_123");
                let [AgentMessageContent::Text { text }] = content.as_slice() else {
                    panic!("expected a single text content item");
                };
                assert_eq!(text, "Hello from your coding assistant.");
                assert_eq!(*phase, Some(MessagePhase::FinalAnswer));
            }
            _ => panic!("expected bridged agent message item"),
        }
    }

    #[test]
    fn bridges_turn_completion_from_server_notifications() {
        let thread_id = "019cee8c-b993-7e33-88c0-014d4e62612d".to_string();
        let turn_id = "019cee8c-b9b4-7f10-a1b0-38caa876a012".to_string();

        let (actual_thread_id, events) = server_notification_thread_events(
            ServerNotification::TurnCompleted(TurnCompletedNotification {
                thread_id: thread_id.clone(),
                turn: Turn {
                    id: turn_id.clone(),
                    items: Vec::new(),
                    status: TurnStatus::Completed,
                    error: None,
                },
            }),
        )
        .expect("notification should bridge");

        assert_eq!(
            actual_thread_id,
            ThreadId::from_string(&thread_id).expect("valid thread id")
        );
        let [event] = events.as_slice() else {
            panic!("expected one bridged event");
        };
        assert_eq!(event.id, String::new());
        let EventMsg::TurnComplete(completed) = &event.msg else {
            panic!("expected turn complete event");
        };
        assert_eq!(completed.turn_id, turn_id);
        assert_eq!(completed.last_agent_message, None);
    }

    #[test]
    fn bridges_text_deltas_from_server_notifications() {
        let thread_id = "019cee8c-b993-7e33-88c0-014d4e62612d".to_string();

        let (_, agent_events) = server_notification_thread_events(
            ServerNotification::AgentMessageDelta(AgentMessageDeltaNotification {
                thread_id: thread_id.clone(),
                turn_id: "turn".to_string(),
                item_id: "item".to_string(),
                delta: "Hello".to_string(),
            }),
        )
        .expect("notification should bridge");
        let [agent_event] = agent_events.as_slice() else {
            panic!("expected one bridged agent delta event");
        };
        assert_eq!(agent_event.id, String::new());
        let EventMsg::AgentMessageDelta(delta) = &agent_event.msg else {
            panic!("expected bridged agent message delta");
        };
        assert_eq!(delta.delta, "Hello");

        let (_, reasoning_events) = server_notification_thread_events(
            ServerNotification::ReasoningSummaryTextDelta(ReasoningSummaryTextDeltaNotification {
                thread_id,
                turn_id: "turn".to_string(),
                item_id: "item".to_string(),
                delta: "Thinking".to_string(),
                summary_index: 0,
            }),
        )
        .expect("notification should bridge");
        let [reasoning_event] = reasoning_events.as_slice() else {
            panic!("expected one bridged reasoning delta event");
        };
        assert_eq!(reasoning_event.id, String::new());
        let EventMsg::AgentReasoningDelta(delta) = &reasoning_event.msg else {
            panic!("expected bridged reasoning delta");
        };
        assert_eq!(delta.delta, "Thinking");
    }

    #[test]
    fn bridges_thread_snapshot_turns_for_resume_restore() {
        let thread_id = ThreadId::new();
        let events = thread_snapshot_events(&Thread {
            id: thread_id.to_string(),
            preview: "hello".to_string(),
            ephemeral: false,
            model_provider: "openai".to_string(),
            created_at: 0,
            updated_at: 0,
            status: ThreadStatus::Idle,
            path: None,
            cwd: PathBuf::from("/tmp/project"),
            cli_version: "test".to_string(),
            source: SessionSource::Cli.into(),
            agent_nickname: None,
            agent_role: None,
            git_info: None,
            name: Some("restore".to_string()),
            turns: vec![
                Turn {
                    id: "turn-complete".to_string(),
                    items: vec![
                        ThreadItem::UserMessage {
                            id: "user-1".to_string(),
                            content: vec![codex_app_server_protocol::UserInput::Text {
                                text: "hello".to_string(),
                                text_elements: Vec::new(),
                            }],
                        },
                        ThreadItem::AgentMessage {
                            id: "assistant-1".to_string(),
                            text: "hi".to_string(),
                            phase: Some(MessagePhase::FinalAnswer),
                        },
                    ],
                    status: TurnStatus::Completed,
                    error: None,
                },
                Turn {
                    id: "turn-interrupted".to_string(),
                    items: Vec::new(),
                    status: TurnStatus::Interrupted,
                    error: None,
                },
            ],
        });

        assert_eq!(events.len(), 6);
        assert!(matches!(events[0].msg, EventMsg::TurnStarted(_)));
        assert!(matches!(events[1].msg, EventMsg::ItemCompleted(_)));
        assert!(matches!(events[2].msg, EventMsg::ItemCompleted(_)));
        assert!(matches!(events[3].msg, EventMsg::TurnComplete(_)));
        assert!(matches!(events[4].msg, EventMsg::TurnStarted(_)));
        let EventMsg::TurnAborted(TurnAbortedEvent { turn_id, reason }) = &events[5].msg else {
            panic!("expected interrupted turn replay");
        };
        assert_eq!(turn_id.as_deref(), Some("turn-interrupted"));
        assert_eq!(*reason, TurnAbortReason::Interrupted);
    }

    #[test]
    fn bridges_remote_command_approval_requests_into_exec_events() {
        let thread_id = ThreadId::new();
        let request = ServerRequest::CommandExecutionRequestApproval {
            request_id: AppServerRequestId::String("req-1".to_string()),
            params: CommandExecutionRequestApprovalParams {
                thread_id: thread_id.to_string(),
                turn_id: "turn-1".to_string(),
                item_id: "item-1".to_string(),
                approval_id: Some("approval-1".to_string()),
                reason: Some("needs write access".to_string()),
                network_approval_context: None,
                command: Some("cargo build --release".to_string()),
                cwd: Some(PathBuf::from("/tmp/rupro")),
                command_actions: None,
                additional_permissions: None,
                skill_metadata: None,
                proposed_execpolicy_amendment: None,
                proposed_network_policy_amendments: Some(vec![
                    codex_app_server_protocol::NetworkPolicyAmendment {
                        host: "crates.io".to_string(),
                        action: codex_app_server_protocol::NetworkPolicyRuleAction::Allow,
                    },
                ]),
                available_decisions: Some(vec![
                    CommandExecutionApprovalDecision::AcceptForSession,
                    CommandExecutionApprovalDecision::ApplyNetworkPolicyAmendment {
                        network_policy_amendment:
                            codex_app_server_protocol::NetworkPolicyAmendment {
                                host: "crates.io".to_string(),
                                action: codex_app_server_protocol::NetworkPolicyRuleAction::Allow,
                            },
                    },
                    CommandExecutionApprovalDecision::Cancel,
                ]),
            },
        };

        let (actual_thread_id, event) =
            server_request_thread_event(&request).expect("request should bridge");

        assert_eq!(actual_thread_id, thread_id);
        let EventMsg::ExecApprovalRequest(request) = event.msg else {
            panic!("expected bridged exec approval event");
        };
        assert_eq!(request.call_id, "item-1");
        assert_eq!(request.approval_id.as_deref(), Some("approval-1"));
        assert_eq!(request.turn_id, "turn-1");
        assert_eq!(
            request.command,
            vec![
                "cargo".to_string(),
                "build".to_string(),
                "--release".to_string()
            ]
        );
        assert_eq!(request.cwd, PathBuf::from("/tmp/rupro"));
        assert_eq!(request.reason.as_deref(), Some("needs write access"));
        assert_eq!(
            request.available_decisions,
            Some(vec![
                ReviewDecision::ApprovedForSession,
                ReviewDecision::NetworkPolicyAmendment {
                    network_policy_amendment: codex_protocol::protocol::NetworkPolicyAmendment {
                        host: "crates.io".to_string(),
                        action: NetworkPolicyRuleAction::Allow,
                    },
                },
                ReviewDecision::Abort,
            ])
        );
    }

    #[test]
    fn bridges_remote_permissions_requests_into_request_permissions_events() {
        let thread_id = ThreadId::new();
        let request = ServerRequest::PermissionsRequestApproval {
            request_id: AppServerRequestId::String("req-2".to_string()),
            params: PermissionsRequestApprovalParams {
                thread_id: thread_id.to_string(),
                turn_id: "turn-2".to_string(),
                item_id: "perm-1".to_string(),
                reason: Some("build needs Cargo target writes".to_string()),
                permissions: codex_app_server_protocol::AdditionalPermissionProfile {
                    network: Some(codex_app_server_protocol::AdditionalNetworkPermissions {
                        enabled: Some(true),
                    }),
                    file_system: Some(codex_app_server_protocol::AdditionalFileSystemPermissions {
                        read: Some(vec![
                            AbsolutePathBuf::from_absolute_path("/tmp/rupro")
                                .expect("absolute path"),
                        ]),
                        write: Some(vec![
                            AbsolutePathBuf::from_absolute_path("/tmp/rupro/target")
                                .expect("absolute path"),
                        ]),
                    }),
                    macos: None,
                },
            },
        };

        let (actual_thread_id, event) =
            server_request_thread_event(&request).expect("request should bridge");

        assert_eq!(actual_thread_id, thread_id);
        let EventMsg::RequestPermissions(request) = event.msg else {
            panic!("expected bridged permissions event");
        };
        assert_eq!(request.call_id, "perm-1");
        assert_eq!(request.turn_id, "turn-2");
        assert_eq!(
            request.reason.as_deref(),
            Some("build needs Cargo target writes")
        );
        assert_eq!(
            request.permissions.network.map(|network| network.enabled),
            Some(Some(true))
        );
        assert_eq!(
            request
                .permissions
                .file_system
                .map(|file_system| file_system.write),
            Some(Some(vec![
                AbsolutePathBuf::from_absolute_path("/tmp/rupro/target").expect("absolute path"),
            ]))
        );
    }
}
