use std::{path::PathBuf, sync::Arc};

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{CommandId, EffectRecovery, SessionId};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

mod actions;
mod contract;
pub(super) mod output;
#[cfg(test)]
mod tests;

use super::{
    PluginManager,
    api::{PLUGIN_API_REVISION, PluginInvocation, PluginOutcome, PluginRequest},
};
use crate::mcp::oauth_operation_id;
use serde::Deserialize as _;

use contract::manage_tool_spec;

use output::{plugin_error, remote_mcp_error_output};
use renoa_kernel::AgentId;

const TOOL_NAME: &str = crate::capabilities::PLUGIN_MANAGE;
const BINDING_REVISION: &str = PLUGIN_API_REVISION;

pub(crate) fn agent_plugin_binding(
    agent_id: AgentId,
    manager: PluginManager,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
) -> AgentToolBinding {
    AgentToolBinding::new(
        BINDING_REVISION,
        Arc::new(ManageTool::for_session(
            agent_id, manager, workspace, session_id, command_id,
        )),
        EffectRecovery::SafeToReplay,
    )
}

struct ManageTool {
    agent_id: AgentId,
    manager: PluginManager,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
    spec: ToolSpec,
}

impl ManageTool {
    fn for_session(
        agent_id: AgentId,
        manager: PluginManager,
        workspace: PathBuf,
        session_id: SessionId,
        command_id: Option<CommandId>,
    ) -> Self {
        Self {
            agent_id,
            manager,
            workspace,
            session_id,
            command_id,
            spec: manage_tool_spec(TOOL_NAME),
        }
    }

    #[cfg(test)]
    fn new(agent_id: AgentId, manager: PluginManager, workspace: PathBuf) -> Self {
        Self::for_session(
            agent_id,
            manager,
            workspace,
            SessionId::new(),
            Some(CommandId::new()),
        )
    }
}

impl Tool for ManageTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.name != TOOL_NAME {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{TOOL_NAME}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let operation_id = oauth_operation_id(self.session_id, self.command_id, &call.id);
            let request = parse(&call.arguments)?;
            let mutating = mutates(&request);
            let outcome = invoke(
                &self.manager,
                self.agent_id,
                &self.workspace,
                request,
                &operation_id,
                cancellation.clone(),
                Some(&updates),
            )
            .await;
            // A call that showed the person a setup link ends the turn there.
            let expires_at_ms = match stopped_by(&outcome) {
                Some(super::PluginError::Mcp(crate::mcp::McpHostError::AwaitingPerson {
                    expires_at_ms,
                })) => *expires_at_ms,
                _ => return render(outcome, mutating),
            };
            // With no runner in this process to finish it, the turn waits for
            // the person on the setup the call just saved.
            if !self.manager.setups().attached() {
                let outcome = invoke(
                    &self.manager,
                    self.agent_id,
                    &self.workspace,
                    parse(&call.arguments)?,
                    &operation_id,
                    cancellation,
                    None,
                )
                .await;
                return render(outcome, mutating);
            }
            self.hand_off(call.arguments, operation_id, expires_at_ms)
                .await
        })
    }
}

impl ManageTool {
    /// Saves a call that showed the person a setup link, so the Host's setup
    /// waits finish it, and tells the agent to end its turn. The call is saved
    /// before the agent hears it is waiting.
    async fn hand_off(
        &self,
        arguments: serde_json::Value,
        operation_id: String,
        expires_at_ms: Option<i64>,
    ) -> Result<ToolOutput, ToolError> {
        let created_at_ms = crate::TurnObservation::now()
            .map_err(|error| ToolError::internal(error.to_string()))?
            .unix_milliseconds();
        let connection = arguments
            .get("connection")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let setup = crate::host::plugin_setups::Setup {
            operation_id,
            agent: self.agent_id,
            session: self.session_id,
            arguments,
            expires_at_ms,
            created_at_ms,
        };
        let database = self.manager.database().to_path_buf();
        let saved = tokio::task::spawn_blocking(move || {
            crate::host::plugin_setups::record(&database, &setup)
        })
        .await
        .map_err(|error| ToolError::internal(error.to_string()))?
        .map_err(|_| {
            ToolError::unavailable(
                "The person was sent a setup link, but Renoa could not save the wait, so their answer would not reach you. Call plugin_manage again with the same arguments.",
            )
        })?;
        if !saved {
            return Err(ToolError::invalid_input(
                "This tool call ID already saved a different plugin_manage call. Make the call again as a new tool call.",
            ));
        }
        self.manager.setups().wake();
        output::json_output(&WaitingOutput {
            status: "waiting_for_person",
            connection,
            expires_at_ms,
            next_action: "The person has the setup link. Tell them in one sentence, then end your turn; do not call plugin_manage for this again. When they finish, or the link expires, the result comes back to you here as a new message.",
        })
    }

    #[cfg(test)]
    async fn list(&self, cursor: Option<&str>, limit: usize) -> Result<ToolOutput, ToolError> {
        let outcome = invoke(
            &self.manager,
            self.agent_id,
            &self.workspace,
            PluginRequest::List {
                cursor: cursor.map(str::to_owned),
                limit,
            },
            "fixture-list",
            CancellationToken::new(),
            None,
        )
        .await;
        render(outcome, false)
    }
}

fn parse(arguments: &serde_json::Value) -> Result<PluginRequest, ToolError> {
    PluginRequest::deserialize(arguments).map_err(|error| {
        ToolError::invalid_input(format!(
            "{TOOL_NAME} arguments are invalid for the selected action: {error}"
        ))
    })
}

fn mutates(request: &PluginRequest) -> bool {
    !matches!(
        request,
        PluginRequest::Inspect { .. } | PluginRequest::List { .. }
    )
}

async fn invoke(
    manager: &PluginManager,
    agent_id: AgentId,
    workspace: &std::path::Path,
    request: PluginRequest,
    operation_id: &str,
    cancellation: CancellationToken,
    updates: Option<&ToolUpdates>,
) -> Result<PluginOutcome, super::PluginError> {
    manager
        .invoke(
            &agent_id,
            workspace,
            request,
            PluginInvocation {
                operation_id,
                updates,
                cancellation,
            },
        )
        .await
}

fn render(
    outcome: Result<PluginOutcome, super::PluginError>,
    mutating: bool,
) -> Result<ToolOutput, ToolError> {
    match outcome {
        Ok(outcome) => actions::render(outcome),
        Err(super::PluginError::Mcp(crate::mcp::McpHostError::Adapter(
            crate::mcp::McpAdapterError::Remote(remote),
        ))) => remote_mcp_error_output(&remote),
        Err(error) => Err(plugin_error(error, mutating)),
    }
}

/// What stopped `outcome`, when it stopped: its own error, or the error of an
/// install whose connection failed.
fn stopped_by(outcome: &Result<PluginOutcome, super::PluginError>) -> Option<&super::PluginError> {
    match outcome {
        Err(error) => Some(error),
        Ok(PluginOutcome::Added(added)) => match &added.connection {
            super::PluginConnectionOutcome::Failed { error, .. } => Some(error),
            _ => None,
        },
        Ok(_) => None,
    }
}

/// Replays a call the Host saved when it handed a setup link to the person,
/// with no one to show a link to. Its rendered result is `Err` when the call,
/// or the connection it installed, did not finish; a sign-in that needs a link
/// of its own asks for the call again, which shows it.
pub(crate) async fn replay_manage_call(
    manager: &PluginManager,
    agent_id: AgentId,
    workspace: &std::path::Path,
    arguments: &serde_json::Value,
    operation_id: &str,
    cancellation: CancellationToken,
) -> Result<String, String> {
    let request = parse(arguments).map_err(|error| error.to_string())?;
    let mutating = mutates(&request);
    let outcome = invoke(
        manager,
        agent_id,
        workspace,
        request,
        operation_id,
        cancellation,
        None,
    )
    .await;
    let stopped = stopped_by(&outcome);
    if let Some(super::PluginError::Mcp(crate::mcp::McpHostError::OAuth(
        crate::mcp::McpOAuthError::AuthorizationRequired(connection),
    ))) = stopped
    {
        return Err(format!(
            "The person finished this step, and signing in to '{connection}' needs a link of its own. Call plugin_manage again with the same arguments to show it."
        ));
    }
    let stopped = stopped.is_some();
    let output = render(outcome, mutating).map_err(|error| error.to_string())?;
    let text = output
        .content
        .iter()
        .filter_map(|block| {
            if let renoa_agent::ContentBlock::Text { text } = block {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if stopped { Err(text) } else { Ok(text) }
}

#[derive(Serialize)]
struct WaitingOutput {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at_ms: Option<i64>,
    next_action: &'static str,
}

#[derive(Serialize)]
struct ConnectedOutput<'a> {
    status: &'static str,
    source: &'static str,
    package_digest: &'a str,
    connection: &'a str,
    server: &'a str,
    catalog_digest: &'a str,
    tools: usize,
    rejected_tools: usize,
    notices: &'a [super::PluginNotice],
    skills: &'a crate::skills::SkillComponentReport,
    activation: &'a crate::plugins::PluginActivation,
}

#[derive(Serialize)]
struct InstalledOutput<'a> {
    status: &'static str,
    source: &'static str,
    package_digest: &'a str,
    metadata: &'a super::PluginMetadata,
    mcp_servers: &'a [super::PluginMcpServer],
    notices: &'a [super::PluginNotice],
    skills: &'a crate::skills::SkillComponentReport,
    activation: &'a crate::plugins::PluginActivation,
}

#[derive(Serialize)]
struct ConnectionOutput {
    status: &'static str,
    package_digest: String,
    server: String,
    connection: String,
    catalog_digest: String,
    tools: usize,
    rejected_tools: usize,
}

#[derive(Serialize)]
struct AuthorizedOutput {
    status: &'static str,
    connection: String,
    catalog_digest: String,
    tools: usize,
    rejected_tools: usize,
}

#[derive(Serialize)]
struct DisconnectedOutput {
    status: &'static str,
    connection: String,
    catalog_retained: bool,
    enabled_for_agent: bool,
}

#[derive(Serialize)]
struct EnabledOutput {
    status: &'static str,
    connection: String,
    catalog_retained: bool,
    enabled_for_agent: bool,
}
