use std::path::PathBuf;
use std::sync::Arc;

use codex_app_server_client::ClientSurface;
use codex_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
use codex_app_server_client::InProcessAppServerClient;
use codex_app_server_client::InProcessClientStartArgs;
use codex_app_server_client::InProcessServerEvent;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ConfigWarningNotification;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::SkillsListEntry as ProtocolSkillsListEntry;
use codex_app_server_protocol::SkillsListParams;
use codex_app_server_protocol::SkillsListResponse;
use codex_arg0::Arg0DispatchPaths;
use codex_core::config::Config;
use codex_core::config_loader::CloudRequirementsLoader;
use codex_core::config_loader::LoaderOverrides;
use codex_feedback::CodexFeedback;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ListSkillsResponseEvent;
use tokio::sync::mpsc;
use toml::Value as TomlValue;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::version::CODEX_CLI_VERSION;

#[derive(Clone)]
pub(crate) struct SkillsListBridgeHandle {
    tx: mpsc::UnboundedSender<SkillsListBridgeCommand>,
}

enum SkillsListBridgeCommand {
    ListSkills {
        cwds: Vec<PathBuf>,
        force_reload: bool,
    },
}

pub(crate) async fn start_skills_list_bridge(
    config: Config,
    arg0_paths: Arg0DispatchPaths,
    cli_kv_overrides: Vec<(String, TomlValue)>,
    cloud_requirements: CloudRequirementsLoader,
    app_event_tx: AppEventSender,
) -> Option<SkillsListBridgeHandle> {
    let config_warnings: Vec<ConfigWarningNotification> = config
        .startup_warnings
        .iter()
        .map(|warning| ConfigWarningNotification {
            summary: warning.clone(),
            details: None,
            path: None,
            range: None,
        })
        .collect();

    let client = match InProcessAppServerClient::start(InProcessClientStartArgs {
        arg0_paths,
        config: Arc::new(config),
        cli_overrides: cli_kv_overrides,
        loader_overrides: LoaderOverrides::default(),
        cloud_requirements,
        feedback: CodexFeedback::new(),
        config_warnings,
        surface: ClientSurface::Tui,
        client_name: Some("codex-tui-skills-bridge".to_string()),
        client_version: CODEX_CLI_VERSION.to_string(),
        experimental_api: false,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await
    {
        Ok(client) => client,
        Err(err) => {
            app_event_tx.send(AppEvent::CodexEvent(Event {
                id: String::new(),
                msg: EventMsg::Warning(codex_protocol::protocol::WarningEvent {
                    message: format!("failed to initialize app-server skills bridge: {err}"),
                }),
            }));
            return None;
        }
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut client = client;
        let mut next_request_id = 1i64;

        loop {
            tokio::select! {
                maybe_command = rx.recv() => {
                    let Some(command) = maybe_command else {
                        let _ = client.shutdown().await;
                        break;
                    };

                    match command {
                        SkillsListBridgeCommand::ListSkills { cwds, force_reload } => {
                            let request = ClientRequest::SkillsList {
                                request_id: RequestId::Integer(next_request_id),
                                params: SkillsListParams {
                                    cwds,
                                    force_reload,
                                    per_cwd_extra_user_roots: None,
                                },
                            };
                            next_request_id += 1;

                            match client.request_typed::<SkillsListResponse>(request).await {
                                Ok(response) => {
                                    app_event_tx.send(AppEvent::CodexEvent(Event {
                                        id: String::new(),
                                        msg: EventMsg::ListSkillsResponse(ListSkillsResponseEvent {
                                            skills: response
                                                .data
                                                .into_iter()
                                                .map(skills_list_entry_to_core)
                                                .collect(),
                                        }),
                                    }));
                                }
                                Err(err) => {
                                    app_event_tx.send(AppEvent::CodexEvent(Event {
                                        id: String::new(),
                                        msg: EventMsg::Error(codex_protocol::protocol::ErrorEvent {
                                            message: format!("skills/list via app-server bridge failed: {err}"),
                                            codex_error_info: None,
                                        }),
                                    }));
                                }
                            }
                        }
                    }
                }
                maybe_event = client.next_event() => {
                    let Some(event) = maybe_event else {
                        break;
                    };

                    if let InProcessServerEvent::Lagged { skipped } = event {
                        app_event_tx.send(AppEvent::CodexEvent(Event {
                            id: String::new(),
                            msg: EventMsg::Warning(codex_protocol::protocol::WarningEvent {
                                message: format!(
                                    "app-server skills bridge lagged; dropped {skipped} events"
                                ),
                            }),
                        }));
                    }
                }
            }
        }
    });

    Some(SkillsListBridgeHandle { tx })
}

impl SkillsListBridgeHandle {
    pub(crate) fn list_skills(&self, cwds: Vec<PathBuf>, force_reload: bool) -> bool {
        self.tx
            .send(SkillsListBridgeCommand::ListSkills { cwds, force_reload })
            .is_ok()
    }
}

fn skills_list_entry_to_core(
    entry: ProtocolSkillsListEntry,
) -> codex_protocol::protocol::SkillsListEntry {
    codex_protocol::protocol::SkillsListEntry {
        cwd: entry.cwd,
        skills: entry
            .skills
            .into_iter()
            .map(skill_metadata_to_core)
            .collect(),
        errors: entry
            .errors
            .into_iter()
            .map(|error| codex_protocol::protocol::SkillErrorInfo {
                path: error.path,
                message: error.message,
            })
            .collect(),
    }
}

fn skill_metadata_to_core(
    metadata: codex_app_server_protocol::SkillMetadata,
) -> codex_protocol::protocol::SkillMetadata {
    codex_protocol::protocol::SkillMetadata {
        name: metadata.name,
        description: metadata.description,
        short_description: metadata.short_description,
        interface: metadata.interface.map(skill_interface_to_core),
        dependencies: metadata.dependencies.map(skill_dependencies_to_core),
        path: metadata.path,
        scope: skill_scope_to_core(metadata.scope),
        enabled: metadata.enabled,
    }
}

fn skill_interface_to_core(
    interface: codex_app_server_protocol::SkillInterface,
) -> codex_protocol::protocol::SkillInterface {
    codex_protocol::protocol::SkillInterface {
        display_name: interface.display_name,
        short_description: interface.short_description,
        icon_small: interface.icon_small,
        icon_large: interface.icon_large,
        brand_color: interface.brand_color,
        default_prompt: interface.default_prompt,
    }
}

fn skill_dependencies_to_core(
    dependencies: codex_app_server_protocol::SkillDependencies,
) -> codex_protocol::protocol::SkillDependencies {
    codex_protocol::protocol::SkillDependencies {
        tools: dependencies
            .tools
            .into_iter()
            .map(|tool| codex_protocol::protocol::SkillToolDependency {
                r#type: tool.r#type,
                value: tool.value,
                description: tool.description,
                transport: tool.transport,
                command: tool.command,
                url: tool.url,
            })
            .collect(),
    }
}

fn skill_scope_to_core(
    scope: codex_app_server_protocol::SkillScope,
) -> codex_protocol::protocol::SkillScope {
    match scope {
        codex_app_server_protocol::SkillScope::User => codex_protocol::protocol::SkillScope::User,
        codex_app_server_protocol::SkillScope::Repo => codex_protocol::protocol::SkillScope::Repo,
        codex_app_server_protocol::SkillScope::System => {
            codex_protocol::protocol::SkillScope::System
        }
        codex_app_server_protocol::SkillScope::Admin => codex_protocol::protocol::SkillScope::Admin,
    }
}
