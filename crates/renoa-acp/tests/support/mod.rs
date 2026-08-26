use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use serde_json::{Value, json};

pub(crate) struct AcpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl AcpProcess {
    pub(crate) fn spawn(
        workspace: &std::path::Path,
        data: &std::path::Path,
        bridge: &std::path::Path,
        auth_store: &std::path::Path,
    ) -> Self {
        Self::spawn_with_providers(
            workspace,
            data,
            bridge,
            auth_store,
            "xai",
            "xai",
            "grok-test",
        )
    }

    pub(crate) fn spawn_with_providers(
        workspace: &std::path::Path,
        data: &std::path::Path,
        bridge: &std::path::Path,
        auth_store: &std::path::Path,
        providers: &str,
        default_provider: &str,
        default_model: &str,
    ) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_renoa-agent"))
            .arg("acp")
            .current_dir(workspace)
            .env("RENOA_DATA_DIR", data)
            .env("RENOA_MODEL_BRIDGE", bridge)
            .env("RENOA_MODEL_PROVIDERS", providers)
            .env("RENOA_MODEL_PROVIDER", default_provider)
            .env("RENOA_MODEL", default_model)
            .env("RENOA_MODEL_AUTH_STORE", auth_store)
            .env("RENOA_TEST_STARTED", data.join("model-started"))
            .env("RENOA_TEST_COMPLETED", data.join("model-completed"))
            .env("RENOA_TEST_CONTINUE", data.join("model-continue"))
            .env("RENOA_TEST_INVOKED", data.join("model-invoked"))
            .env("RENOA_TEST_MODEL_ATTEMPTS", data.join("model-attempts"))
            .env(
                "RENOA_TEST_COMPACTION_ATTEMPTS",
                data.join("compaction-attempts"),
            )
            .env("RENOA_TEST_MODEL_CHILD_PID", data.join("model-child-pid"))
            .env("RENOA_TEST_UNSAFE_STARTED", data.join("unsafe-started"))
            .env("RENOA_TEST_UNSAFE_CHILD_PID", data.join("unsafe-child-pid"))
            .env("RENOA_TEST_UNSAFE_CONTINUE", data.join("unsafe-continue"))
            .env("RENOA_TEST_UNSAFE_COMPLETED", data.join("unsafe-completed"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start ACP process");
        let stdin = child.stdin.take().expect("ACP stdin");
        let stdout = BufReader::new(child.stdout.take().expect("ACP stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    pub(crate) fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("open ACP stdin");
        serde_json::to_writer(&mut *stdin, &message).expect("encode ACP request");
        stdin.write_all(b"\n").expect("terminate ACP request");
        stdin.flush().expect("flush ACP request");
    }

    pub(crate) fn initialize(&mut self) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": { "name": "renoa-test", "version": "0.0.0" }
            }
        }));
        self.read()
    }

    pub(crate) fn create_session(&mut self, workspace: &std::path::Path) -> Value {
        self.create_session_with_updates(workspace).1
    }

    pub(crate) fn create_session_with_updates(
        &mut self,
        workspace: &std::path::Path,
    ) -> (Vec<Value>, Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": workspace, "mcpServers": [] }
        }));
        let mut updates = Vec::new();
        loop {
            let message = self.read();
            if message["id"] == 2 {
                return (updates, message);
            }
            updates.push(message);
        }
    }

    pub(crate) fn load_session(
        &mut self,
        workspace: &std::path::Path,
        session_id: &str,
    ) -> (Vec<Value>, Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/load",
            "params": {
                "sessionId": session_id,
                "cwd": workspace,
                "mcpServers": []
            }
        }));
        let mut updates = Vec::new();
        loop {
            let message = self.read();
            if message["id"] == 2 {
                return (updates, message);
            }
            updates.push(message);
        }
    }

    pub(crate) fn prompt(&mut self, session_id: &str, text: &str, turn_id: &str) -> (Value, Value) {
        self.send_prompt(session_id, text, turn_id);
        let mut messages = self.read_until_response(3);
        let response = messages.pop().expect("prompt response");
        let update_position = messages
            .iter()
            .position(|message| {
                message["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
            })
            .expect("simple prompt update");
        let update = messages.remove(update_position);
        assert!(
            messages.len() <= 1
                && messages.iter().all(|message| {
                    message["params"]["update"]["sessionUpdate"] == "usage_update"
                }),
            "simple prompt returned an unexpected ACP message sequence: {messages:?}"
        );
        (update, response)
    }

    pub(crate) fn send_prompt_id(
        &mut self,
        request_id: u64,
        session_id: &str,
        text: &str,
        turn_id: &str,
    ) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": text }],
                "_meta": { "requestId": turn_id, "promptId": turn_id }
            }
        }));
    }

    pub(crate) fn send_prompt(&mut self, session_id: &str, text: &str, turn_id: &str) {
        self.send_prompt_id(3, session_id, text, turn_id);
    }

    pub(crate) fn read_until_response(&mut self, request_id: u64) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            let message = self.read();
            let is_response = message["id"] == request_id;
            messages.push(message);
            if is_response {
                return messages;
            }
        }
    }

    pub(crate) fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read ACP response");
        assert!(!line.is_empty(), "ACP process closed stdout");
        serde_json::from_str(&line).expect("decode ACP response")
    }

    pub(crate) fn finish(mut self) {
        drop(self.stdin.take());
        let output = self.child.wait_with_output().expect("wait for ACP process");
        assert!(
            output.status.success(),
            "ACP process failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[allow(
        dead_code,
        reason = "shared integration-test support is compiled by tests that do not simulate a crash"
    )]
    pub(crate) fn kill(mut self) {
        drop(self.stdin.take());
        self.child.kill().expect("kill ACP process");
        let status = self.child.wait().expect("reap killed ACP process");
        assert!(!status.success(), "killed ACP process exited successfully");
    }
}

pub(crate) const BRIDGE: &str = include_str!("bridge.js");
