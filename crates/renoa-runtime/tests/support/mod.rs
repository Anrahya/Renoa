use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use futures_util::{StreamExt, stream};
use renoa_core::{
    BoxFuture, CapabilityCall, CapabilityHost, CapabilityOutcome, CapabilityRequest,
    CapabilitySpec, CommandEnvelope, CommandId, CommandInput, ModelDriver, ModelError, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, PrincipalId, ResolvedAgent, RunEventKind,
    SurfaceRef, TargetRef,
};
use serde_json::json;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub enum ModelStep {
    Respond(ModelResponse),
    Fail(String),
    Pending,
}

pub struct ScriptedModel {
    steps: Mutex<VecDeque<ModelStep>>,
    requests: Mutex<Vec<ModelRequest>>,
    request_started: Notify,
}

impl ScriptedModel {
    pub fn new(responses: Vec<ModelResponse>) -> Self {
        Self::from_steps(responses.into_iter().map(ModelStep::Respond).collect())
    }

    pub fn from_steps(steps: Vec<ModelStep>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
            requests: Mutex::new(Vec::new()),
            request_started: Notify::new(),
        }
    }

    pub fn requests(&self) -> Vec<ModelRequest> {
        self.requests
            .lock()
            .expect("scripted model request lock must not be poisoned")
            .clone()
    }

    pub async fn wait_for_request(&self) {
        self.request_started.notified().await;
    }
}

impl ModelDriver for ScriptedModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests
            .lock()
            .expect("scripted model request lock must not be poisoned")
            .push(request);
        self.request_started.notify_one();
        let step = self
            .steps
            .lock()
            .expect("scripted model step lock must not be poisoned")
            .pop_front()
            .unwrap_or_else(|| ModelStep::Fail("scripted model ran out of steps".to_owned()));
        match step {
            ModelStep::Respond(response) => {
                stream::once(async { Ok(ModelEvent::Completed { response }) }).boxed()
            }
            ModelStep::Fail(error) => stream::once(async { Err(ModelError::new(error)) }).boxed(),
            ModelStep::Pending => stream::pending().boxed(),
        }
    }
}

pub struct TestCapabilityHost {
    root: PathBuf,
    executions: Mutex<Vec<String>>,
}

impl TestCapabilityHost {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            executions: Mutex::new(Vec::new()),
        }
    }

    pub fn executions(&self) -> Vec<String> {
        self.executions
            .lock()
            .expect("capability execution lock must not be poisoned")
            .clone()
    }

    fn read_file(&self, request: &CapabilityRequest) -> CapabilityOutcome {
        let Some(path) = string_argument(request, "path") else {
            return CapabilityOutcome::error("missing path");
        };
        match fs::read_to_string(self.root.join(path)) {
            Ok(content) => success(json!({ "content": content })),
            Err(error) => CapabilityOutcome::error(error.to_string()),
        }
    }

    fn edit_file(&self, request: &CapabilityRequest) -> CapabilityOutcome {
        let Some(path) = string_argument(request, "path") else {
            return CapabilityOutcome::error("missing path");
        };
        let Some(old_text) = string_argument(request, "oldText") else {
            return CapabilityOutcome::error("missing oldText");
        };
        let Some(new_text) = string_argument(request, "newText") else {
            return CapabilityOutcome::error("missing newText");
        };
        let path = self.root.join(path);
        let Ok(content) = fs::read_to_string(&path) else {
            return CapabilityOutcome::error("fixture file is unreadable");
        };
        match fs::write(path, content.replacen(old_text, new_text, 1)) {
            Ok(()) => success(json!({ "changed": true })),
            Err(error) => CapabilityOutcome::error(error.to_string()),
        }
    }

    async fn exec(&self, request: &CapabilityRequest) -> CapabilityOutcome {
        let Some(program) = string_argument(request, "program") else {
            return CapabilityOutcome::error("missing program");
        };
        let args = request.call.arguments["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str());
        match tokio::process::Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .output()
            .await
        {
            Ok(output) => CapabilityOutcome {
                model_view: json!({
                    "success": output.status.success(),
                    "stdout": String::from_utf8_lossy(&output.stdout),
                    "stderr": String::from_utf8_lossy(&output.stderr),
                }),
                is_error: !output.status.success(),
            },
            Err(error) => CapabilityOutcome::error(error.to_string()),
        }
    }
}

impl CapabilityHost for TestCapabilityHost {
    fn specs(&self) -> Vec<CapabilitySpec> {
        ["read_file", "edit_file", "exec"]
            .into_iter()
            .map(|name| CapabilitySpec {
                name: name.to_owned(),
                description: name.to_owned(),
                input_schema: json!({ "type": "object" }),
            })
            .collect()
    }

    fn execute(
        &self,
        request: CapabilityRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, CapabilityOutcome> {
        self.executions
            .lock()
            .expect("capability execution lock must not be poisoned")
            .push(request.call.call_id.clone());
        Box::pin(async move {
            match request.call.name.as_str() {
                "read_file" => self.read_file(&request),
                "edit_file" => self.edit_file(&request),
                "exec" => self.exec(&request).await,
                name => CapabilityOutcome::error(format!("unknown capability: {name}")),
            }
        })
    }
}

pub fn scripted_responses() -> Vec<ModelResponse> {
    vec![
        tool_response("read", "read_file", json!({ "path": "hello.txt" })),
        tool_response(
            "edit",
            "edit_file",
            json!({
                "path": "hello.txt",
                "oldText": "first line\n",
                "newText": "first line\nsecond line\n",
            }),
        ),
        tool_response(
            "verify",
            "exec",
            json!({
                "program": "/bin/sh",
                "args": ["-c", "grep -Fx 'second line' hello.txt"],
            }),
        ),
        ModelResponse {
            text: "Added the second line and verified it.".to_owned(),
            capability_calls: Vec::new(),
            truncated: false,
        },
    ]
}

pub fn test_command() -> CommandEnvelope {
    CommandEnvelope {
        command_id: CommandId::new(),
        principal_id: PrincipalId::new(),
        surface: SurfaceRef::new("test"),
        target: TargetRef::new("local:test-workspace"),
        input: CommandInput::Text {
            text: "Read hello.txt, add a second line, verify it, and report back.".to_owned(),
        },
    }
}

pub fn test_agent() -> ResolvedAgent {
    ResolvedAgent {
        instructions: "You are the Renoa test coding agent.".to_owned(),
        capability_grants: vec![
            "read_file".to_owned(),
            "edit_file".to_owned(),
            "exec".to_owned(),
        ],
    }
}

pub fn tool_response(call_id: &str, name: &str, arguments: serde_json::Value) -> ModelResponse {
    ModelResponse {
        text: String::new(),
        capability_calls: vec![CapabilityCall {
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            arguments,
        }],
        truncated: false,
    }
}

pub const fn event_name(event: &RunEventKind) -> &'static str {
    match event {
        RunEventKind::RunStarted { .. } => "run_started",
        RunEventKind::ModelRequested { .. } => "model_requested",
        RunEventKind::ModelResponded { .. } => "model_responded",
        RunEventKind::CapabilityRequested { .. } => "capability_requested",
        RunEventKind::CapabilityCompleted { .. } => "capability_completed",
        RunEventKind::RunTerminated { .. } => "run_terminated",
    }
}

fn string_argument<'a>(request: &'a CapabilityRequest, name: &str) -> Option<&'a str> {
    request.call.arguments[name].as_str()
}

fn success(model_view: serde_json::Value) -> CapabilityOutcome {
    CapabilityOutcome {
        model_view,
        is_error: false,
    }
}
