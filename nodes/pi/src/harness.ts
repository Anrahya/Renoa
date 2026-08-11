import {
  Agent,
  type AgentMessage,
  type AgentTool,
  type StreamFn,
} from "@earendil-works/pi-agent-core";
import { contentText, type Api, type Model } from "@earendil-works/pi-ai";

import type { ExecutionTerminal, QueuedExecution } from "./protocol.js";
import { NodeState } from "./state.js";
import { createWorkspaceTools, type WorkspaceConfig } from "./workspace-tools.js";

export interface PiHarnessOptions {
  readonly model: Model<Api>;
  readonly streamFn: StreamFn;
  readonly instructions: string;
  readonly target: string;
  readonly workspace?: WorkspaceConfig;
}

export class PiHarness {
  readonly #instructions: string;
  readonly #model: Model<Api>;
  readonly #streamFn: StreamFn;
  readonly #target: string;
  readonly #tools: readonly AgentTool[];

  constructor(options: PiHarnessOptions) {
    this.#instructions = options.instructions;
    this.#model = options.model;
    this.#streamFn = options.streamFn;
    this.#tools = options.workspace === undefined ? [] : createWorkspaceTools(options.workspace);
    this.#target = options.target;
  }

  async execute(
    execution: QueuedExecution,
    state: NodeState,
    signal: AbortSignal,
  ): Promise<void> {
    if (execution.target !== this.#target) {
      state.finish(
        execution.commandId,
        { status: "failed", error: "Pi harness is not bound to the command target" },
        state.loadMessages<AgentMessage>(execution.taskId),
      );
      return;
    }
    const agent = new Agent({
      initialState: {
        systemPrompt: this.#instructions,
        model: this.#model,
        thinkingLevel: "off",
        tools: [...this.#tools],
        messages: state.loadMessages<AgentMessage>(execution.taskId),
      },
      streamFn: this.#streamFn,
      sessionId: execution.taskId,
    });
    agent.subscribe((event) => {
      switch (event.type) {
        case "turn_start":
          state.appendEvent(execution.commandId, { type: "turn_started" });
          break;
        case "message_end":
          if (event.message.role === "assistant") {
            state.appendEvent(execution.commandId, {
              type: "assistant_message",
              text: contentText(event.message.content),
            });
          }
          break;
        case "tool_execution_start":
          state.appendEvent(execution.commandId, {
            type: "tool_started",
            call_id: event.toolCallId,
            name: event.toolName,
            arguments: event.args,
          });
          break;
        case "tool_execution_end":
          state.appendEvent(execution.commandId, {
            type: "tool_finished",
            call_id: event.toolCallId,
            output: contentText(event.result.content),
            is_error: event.isError,
          });
          break;
        case "agent_start":
        case "agent_end":
        case "turn_end":
        case "message_start":
        case "message_update":
        case "tool_execution_update":
          break;
      }
    });

    if (signal.aborted) {
      state.finish(
        execution.commandId,
        { status: "cancelled", reason: abortReason(signal) },
        agent.state.messages,
      );
      return;
    }
    const abort = () => agent.abort();
    signal.addEventListener("abort", abort, { once: true });
    try {
      await agent.prompt(execution.text);
    } finally {
      signal.removeEventListener("abort", abort);
    }
    state.finish(execution.commandId, terminalState(agent.state.messages), agent.state.messages);
  }
}

function terminalState(messages: readonly AgentMessage[]): ExecutionTerminal {
  const assistant = messages.findLast((message) => message.role === "assistant");
  if (assistant === undefined || assistant.role !== "assistant") {
    return { status: "failed", error: "Pi produced no assistant message" };
  }
  if (assistant.stopReason === "aborted") {
    return {
      status: "cancelled",
      reason: assistant.errorMessage ?? "Pi execution was aborted",
    };
  }
  if (assistant.stopReason === "error") {
    return {
      status: "failed",
      error: assistant.errorMessage ?? "Pi model request failed",
    };
  }
  return { status: "completed" };
}

function abortReason(signal: AbortSignal): string {
  return signal.reason instanceof Error ? signal.reason.message : "Pi execution was cancelled";
}
