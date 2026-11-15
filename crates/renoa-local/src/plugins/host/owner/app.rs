//! `app_manage`: start, stop, restart, inspect, and tail logs of a supervised
//! live web app in the agent's workspace, served by the node's site gateway at
//! `/apps/<name>/`.

use std::sync::Arc;

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::OwnerTurn;
use crate::apps::{AppError, Apps, LiveApp};

#[cfg(test)]
mod tests;

pub(crate) const MANAGE: &str = "app_manage";

pub(super) struct AppTool {
    pub(super) turn: Arc<OwnerTurn>,
    pub(super) spec: ToolSpec,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    action: String,
    name: Option<String>,
    folder: Option<String>,
    command: Option<String>,
}

enum Action {
    Start {
        name: String,
        folder: String,
        command: String,
    },
    Stop {
        name: String,
    },
    Restart {
        name: String,
    },
    Status {
        name: Option<String>,
    },
    Logs {
        name: String,
    },
}

pub(super) fn spec() -> ToolSpec {
    ToolSpec {
        name: MANAGE.to_owned(),
        description: format!(
            "Start, stop, restart, inspect status, or read logs for a supervised live web app backed by a subdirectory in your workspace and one command listening on 127.0.0.1:$PORT. The app runs in an isolated network/mount sandbox (writable only to its own folder and /tmp, no access to credentials, state, or host loopback services), survives node restarts and reboots, stops after {} minutes idle, and wakes on the first request at <origin>/apps/<name>/.",
            crate::apps::IDLE_TIMEOUT_MINUTES
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "stop", "restart", "status", "logs"]
                },
                "name": {
                    "type": "string",
                    "description": "App address segment at <origin>/apps/<name>/: 1 to 63 lowercase letters, digits, or hyphens, starting and ending with a letter or number."
                },
                "folder": {
                    "type": "string",
                    "description": "Workspace-relative path to the app folder (required for action=\"start\")."
                },
                "command": {
                    "type": "string",
                    "description": "Command executed in /app that listens on 127.0.0.1:$PORT and reads $BASE_PATH (required for action=\"start\")."
                }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
    }
}

impl Tool for AppTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.name != MANAGE {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{MANAGE}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let action = parse(&call.arguments)?;
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled(
                    format!("{MANAGE} was cancelled before changing anything"),
                    false,
                ));
            }
            tokio::select! {
                biased;
                () = cancellation.cancelled() => Err(ToolError::cancelled(
                    format!("{MANAGE} was cancelled while running"),
                    false,
                )),
                res = run(&self.turn, action) => res,
            }
        })
    }
}

fn parse(arguments: &Value) -> Result<Action, ToolError> {
    let args: Arguments = serde_json::from_value(arguments.clone())
        .map_err(|err| ToolError::invalid_input(format!("{MANAGE}: {err}")))?;
    match args.action.as_str() {
        "start" => match (args.name, args.folder, args.command) {
            (Some(name), Some(folder), Some(command)) => Ok(Action::Start {
                name,
                folder,
                command,
            }),
            _ => Err(ToolError::invalid_input(
                r#"app_manage action "start" requires "name", "folder" (workspace-relative path), and "command" listening on 127.0.0.1:$PORT. Example: {"action": "start", "name": "demo", "folder": "app", "command": "python3 server.py"}"#,
            )),
        },
        "stop" => {
            if args.folder.is_some() || args.command.is_some() {
                return Err(ToolError::invalid_input(
                    r#"app_manage action "stop" takes "name" only. Example: {"action": "stop", "name": "demo"}"#,
                ));
            }
            args.name.map(|name| Action::Stop { name }).ok_or_else(|| {
                ToolError::invalid_input(
                    r#"app_manage action "stop" requires "name". Example: {"action": "stop", "name": "demo"}"#,
                )
            })
        }
        "restart" => {
            if args.folder.is_some() || args.command.is_some() {
                return Err(ToolError::invalid_input(
                    r#"app_manage action "restart" takes "name" only. Example: {"action": "restart", "name": "demo"}"#,
                ));
            }
            args.name
                .map(|name| Action::Restart { name })
                .ok_or_else(|| {
                    ToolError::invalid_input(
                        r#"app_manage action "restart" requires "name". Example: {"action": "restart", "name": "demo"}"#,
                    )
                })
        }
        "status" => {
            if args.folder.is_some() || args.command.is_some() {
                return Err(ToolError::invalid_input(
                    r#"app_manage action "status" takes an optional "name" only. Example: {"action": "status", "name": "demo"}"#,
                ));
            }
            Ok(Action::Status { name: args.name })
        }
        "logs" => {
            if args.folder.is_some() || args.command.is_some() {
                return Err(ToolError::invalid_input(
                    r#"app_manage action "logs" takes "name" only. Example: {"action": "logs", "name": "demo"}"#,
                ));
            }
            args.name.map(|name| Action::Logs { name }).ok_or_else(|| {
                ToolError::invalid_input(
                    r#"app_manage action "logs" requires "name". Example: {"action": "logs", "name": "demo"}"#,
                )
            })
        }
        other => Err(ToolError::invalid_input(format!(
            r#"unknown app_manage action "{other}": choose "start", "stop", "restart", "status", or "logs""#
        ))),
    }
}

async fn run(turn: &OwnerTurn, action: Action) -> Result<ToolOutput, ToolError> {
    let origin = turn.site_origin.as_deref().ok_or_else(|| {
        ToolError::unavailable(
            "Live apps are not enabled on this Host: no site gateway is configured.",
        )
    })?;
    let apps = Apps::from_database(&turn.database);
    let text = match action {
        Action::Start {
            name,
            folder,
            command,
        } => {
            let app = apps
                .start(
                    turn.agent,
                    turn.principal,
                    &turn.workspace,
                    &name,
                    &folder,
                    &command,
                )
                .await
                .map_err(|err| map_error("start", &err))?;
            format_app_summary("Started", &app, origin)
        }
        Action::Stop { name } => {
            let app = apps
                .stop_for(&name, turn.principal)
                .await
                .map_err(|err| map_error("stop", &err))?;
            format_app_summary("Stopped", &app, origin)
        }
        Action::Restart { name } => {
            let app = apps
                .restart(&name, turn.principal)
                .await
                .map_err(|err| map_error("restart", &err))?;
            format_app_summary("Restarted", &app, origin)
        }
        Action::Status { name: Some(name) } => {
            let app = apps
                .status_for(&name, turn.principal)
                .await
                .map_err(|err| map_error("status", &err))?;
            format_app_summary("Status", &app, origin)
        }
        Action::Status { name: None } => {
            let list = apps
                .list(turn.principal)
                .await
                .map_err(|err| map_error("status", &err))?;
            if list.is_empty() {
                "No live apps are configured.".to_owned()
            } else {
                list.iter()
                    .map(|app| format_app_line(app, origin))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        Action::Logs { name } => {
            let entry = apps
                .logs_for(&name, turn.principal)
                .await
                .map_err(|err| map_error("logs", &err))?;
            let mut out = format_app_summary("Logs", &entry.app, origin);
            if entry.logs.is_empty() {
                out.push_str("\n(no log lines captured)");
            } else {
                out.push_str("\n\n");
                out.push_str(&entry.logs);
            }
            out
        }
    };

    Ok(ToolOutput {
        content: vec![ContentBlock::text(text)],
        details: None,
        is_error: false,
        closing: None,
    })
}

fn app_link(origin: &str, name: &str) -> String {
    let trimmed = origin.trim_end_matches('/');
    format!("[{name}]({trimmed}/apps/{name}/)")
}

fn format_app_summary(verb: &str, app: &LiveApp, origin: &str) -> String {
    let link = app_link(origin, &app.name);
    let mut summary = format!(
        "{verb} {link} (state={}, port=127.0.0.1:{}, command=`{}`)",
        app.state, app.port, app.command
    );
    if let Some(reason) = &app.failure_reason {
        summary.push_str("\nfailure: ");
        summary.push_str(reason);
    }
    summary
}

fn format_app_line(app: &LiveApp, origin: &str) -> String {
    let link = app_link(origin, &app.name);
    match &app.failure_reason {
        Some(reason) => format!(
            "- {link}: state={}, port={}, error={reason}",
            app.state, app.port
        ),
        None => format!("- {link}: state={}, port={}", app.state, app.port),
    }
}

fn map_error(action: &str, err: &AppError) -> ToolError {
    renoa_telemetry::event(
        "renoa.host",
        "warn",
        "app_manage_failed",
        &json!({ "action": action, "error": err.to_string() }),
    );
    match err {
        AppError::InvalidName | AppError::InvalidFolder(_) | AppError::InvalidCommand => {
            ToolError::invalid_input(err.to_string())
        }
        AppError::NotFound => ToolError::not_found(err.to_string()),
        AppError::PermissionDenied => ToolError::permission_denied(err.to_string()),
        AppError::StartupFailed { .. } | AppError::Failed { .. } => {
            ToolError::process_failed(err.to_string(), false)
        }
        AppError::NoAvailablePort
        | AppError::SupervisorUnavailable(_)
        | AppError::Catalog(_)
        | AppError::Io(_) => ToolError::unavailable(err.to_string()),
    }
}
