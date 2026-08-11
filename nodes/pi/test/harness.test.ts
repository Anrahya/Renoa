import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import {
  contentText,
  fauxAssistantMessage,
  fauxProvider,
  fauxToolCall,
} from "@earendil-works/pi-ai";

import { PiHarness } from "../src/harness.js";
import type { ExecuteCommand } from "../src/protocol.js";
import { NodeState } from "../src/state.js";

test("Pi activity and conversation context are durably projected into RCP", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-harness-"));
  try {
    const faux = fauxProvider();
    faux.setResponses([
      (context) => {
        assert.deepEqual(context.tools, []);
        return fauxAssistantMessage("first answer");
      },
      fauxAssistantMessage("second answer"),
    ]);
    const harness = new PiHarness({
      instructions: "Answer clearly.",
      model: faux.getModel(),
      streamFn: faux.provider.streamSimple.bind(faux.provider),
      target: "workspace:renoa",
    });
    const state = new NodeState(join(directory, "node.sqlite"));

    const first = command("00000000-0000-0000-0000-000000000011", "first question");
    state.admit(first);
    const firstExecution = state.claimNext();
    assert.ok(firstExecution);
    await harness.execute(firstExecution, state, new AbortController().signal);

    const second = command("00000000-0000-0000-0000-000000000012", "second question");
    state.admit(second);
    const secondExecution = state.claimNext();
    assert.ok(secondExecution);
    await harness.execute(secondExecution, state, new AbortController().signal);

    const publications = state.pendingPublications();
    assert.deepEqual(
      publications[0]?.events.map((event) => event.kind),
      [
        { type: "execution_started" },
        { type: "turn_started" },
        { type: "assistant_message", text: "first answer" },
        { type: "execution_terminated", terminal: { status: "completed" } },
      ],
    );
    assert.deepEqual(
      publications[1]?.events.map((event) => event.kind),
      [
        { type: "execution_started" },
        { type: "turn_started" },
        { type: "assistant_message", text: "second answer" },
        { type: "execution_terminated", terminal: { status: "completed" } },
      ],
    );
    const messages = state.loadMessages<{ readonly role: string }>(first.taskId);
    assert.deepEqual(
      messages.map((message) => message.role),
      ["user", "assistant", "user", "assistant"],
    );
    state.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("a locally configured read tool reads a file and publishes its activity", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-read-"));
  const workspace = join(directory, "workspace");
  try {
    await mkdir(workspace);
    await writeFile(join(workspace, "hello.txt"), "hello from Renoa\n");
    const faux = fauxProvider();
    faux.setResponses([
      (context) => {
        assert.deepEqual(context.tools?.map((tool) => tool.name), ["read"]);
        return fauxAssistantMessage(
          fauxToolCall("read", { path: "hello.txt" }, { id: "read-1" }),
          { stopReason: "toolUse" },
        );
      },
      (context) => {
        const result = context.messages.findLast((message) => message.role === "toolResult");
        assert.ok(result);
        assert.match(contentText(result.content), /hello from Renoa/);
        return fauxAssistantMessage("The file contains the expected text.");
      },
    ]);
    const harness = new PiHarness({
      instructions: "Answer clearly.",
      model: faux.getModel(),
      streamFn: faux.provider.streamSimple.bind(faux.provider),
      target: "workspace:renoa",
      workspace: { root: workspace, access: "read" },
    });
    const state = new NodeState(join(directory, "node.sqlite"));
    state.admit(command("00000000-0000-0000-0000-000000000020", "read hello.txt"));
    const execution = state.claimNext();
    assert.ok(execution);

    await harness.execute(execution, state, new AbortController().signal);

    const activity = state
      .pendingPublications()[0]?.events.map((event) => event.kind)
      .filter((kind) => kind.type === "tool_started" || kind.type === "tool_finished");
    assert.deepEqual(activity, [
      {
        type: "tool_started",
        call_id: "read-1",
        name: "read",
        arguments: { path: "hello.txt" },
      },
      {
        type: "tool_finished",
        call_id: "read-1",
        output: "hello from Renoa\n",
        is_error: false,
      },
    ]);
    assert.deepEqual(state.pendingPublications()[0]?.events.at(-1)?.kind, {
      type: "execution_terminated",
      terminal: { status: "completed" },
    });
    state.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("a read-write Pi workspace edits a file and publishes its activity", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-edit-"));
  const workspace = join(directory, "workspace");
  try {
    await mkdir(workspace);
    await writeFile(join(workspace, "hello.txt"), "hello from Renoa\n");
    const faux = fauxProvider();
    faux.setResponses([
      (context) => {
        assert.deepEqual(context.tools?.map((tool) => tool.name), ["read", "write", "edit"]);
        return fauxAssistantMessage(
          fauxToolCall(
            "edit",
            {
              path: "hello.txt",
              edits: [{ oldText: "hello", newText: "goodbye" }],
            },
            { id: "edit-1" },
          ),
          { stopReason: "toolUse" },
        );
      },
      fauxAssistantMessage("The file was updated."),
    ]);
    const harness = new PiHarness({
      instructions: "Edit carefully.",
      model: faux.getModel(),
      streamFn: faux.provider.streamSimple.bind(faux.provider),
      target: "workspace:renoa",
      workspace: { root: workspace, access: "read_write" },
    });
    const state = new NodeState(join(directory, "node.sqlite"));
    state.admit(command("00000000-0000-0000-0000-000000000021", "edit hello.txt"));
    const execution = state.claimNext();
    assert.ok(execution);

    await harness.execute(execution, state, new AbortController().signal);

    const activity = state
      .pendingPublications()[0]?.events.map((event) => event.kind)
      .filter((kind) => kind.type === "tool_started" || kind.type === "tool_finished");
    assert.deepEqual(activity, [
      {
        type: "tool_started",
        call_id: "edit-1",
        name: "edit",
        arguments: {
          path: "hello.txt",
          edits: [{ oldText: "hello", newText: "goodbye" }],
        },
      },
      {
        type: "tool_finished",
        call_id: "edit-1",
        output: "Successfully replaced 1 block(s) in hello.txt.",
        is_error: false,
      },
    ]);
    assert.equal(await readFile(join(workspace, "hello.txt"), "utf8"), "goodbye from Renoa\n");
    state.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("a read-write Pi workspace creates a file and publishes its activity", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-write-"));
  const workspace = join(directory, "workspace");
  try {
    await mkdir(workspace);
    const faux = fauxProvider();
    faux.setResponses([
      fauxAssistantMessage(
        fauxToolCall(
          "write",
          { path: "src/answer.ts", content: "export const answer = 42;\n" },
          { id: "write-1" },
        ),
        { stopReason: "toolUse" },
      ),
      fauxAssistantMessage("The file was created."),
    ]);
    const harness = new PiHarness({
      instructions: "Write carefully.",
      model: faux.getModel(),
      streamFn: faux.provider.streamSimple.bind(faux.provider),
      target: "workspace:renoa",
      workspace: { root: workspace, access: "read_write" },
    });
    const state = new NodeState(join(directory, "node.sqlite"));
    state.admit(command("00000000-0000-0000-0000-000000000025", "create src/answer.ts"));
    const execution = state.claimNext();
    assert.ok(execution);

    await harness.execute(execution, state, new AbortController().signal);

    const activity = state
      .pendingPublications()[0]?.events.map((event) => event.kind)
      .filter((kind) => kind.type === "tool_started" || kind.type === "tool_finished");
    assert.deepEqual(activity, [
      {
        type: "tool_started",
        call_id: "write-1",
        name: "write",
        arguments: { path: "src/answer.ts", content: "export const answer = 42;\n" },
      },
      {
        type: "tool_finished",
        call_id: "write-1",
        output: "Successfully wrote 26 bytes to src/answer.ts",
        is_error: false,
      },
    ]);
    assert.equal(
      await readFile(join(workspace, "src/answer.ts"), "utf8"),
      "export const answer = 42;\n",
    );
    state.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("the local read tool rejects parent traversal", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-read-boundary-"));
  const workspace = join(directory, "workspace");
  try {
    await mkdir(workspace);
    await writeFile(join(directory, "secret.txt"), "outside secret\n");
    const faux = fauxProvider();
    faux.setResponses([
      fauxAssistantMessage(fauxToolCall("read", { path: "../secret.txt" }, { id: "read-2" }), {
        stopReason: "toolUse",
      }),
      (context) => {
        const result = context.messages.findLast((message) => message.role === "toolResult");
        assert.ok(result);
        assert.equal(result.isError, true);
        assert.equal(contentText(result.content), "path escapes the bound workspace");
        return fauxAssistantMessage("The file is outside the authorized workspace.");
      },
    ]);
    const harness = new PiHarness({
      instructions: "Answer clearly.",
      model: faux.getModel(),
      streamFn: faux.provider.streamSimple.bind(faux.provider),
      target: "workspace:renoa",
      workspace: { root: workspace, access: "read" },
    });
    const state = new NodeState(join(directory, "node.sqlite"));
    state.admit(command("00000000-0000-0000-0000-000000000022", "read ../secret.txt"));
    const execution = state.claimNext();
    assert.ok(execution);

    await harness.execute(execution, state, new AbortController().signal);

    const finished = state
      .pendingPublications()[0]?.events.map((event) => event.kind)
      .find((kind) => kind.type === "tool_finished");
    assert.deepEqual(finished, {
      type: "tool_finished",
      call_id: "read-2",
      output: "path escapes the bound workspace",
      is_error: true,
    });
    state.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("the local read tool rejects a symlink that leaves the workspace", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-read-symlink-"));
  const workspace = join(directory, "workspace");
  try {
    await mkdir(workspace);
    await writeFile(join(directory, "secret.txt"), "outside secret\n");
    await symlink("../secret.txt", join(workspace, "link.txt"));
    const faux = fauxProvider();
    faux.setResponses([
      fauxAssistantMessage(fauxToolCall("read", { path: "link.txt" }, { id: "read-3" }), {
        stopReason: "toolUse",
      }),
      (context) => {
        const result = context.messages.findLast((message) => message.role === "toolResult");
        assert.ok(result);
        assert.equal(result.isError, true);
        assert.equal(contentText(result.content), "path escapes the bound workspace");
        return fauxAssistantMessage("The link is outside the authorized workspace.");
      },
    ]);
    const harness = new PiHarness({
      instructions: "Answer clearly.",
      model: faux.getModel(),
      streamFn: faux.provider.streamSimple.bind(faux.provider),
      target: "workspace:renoa",
      workspace: { root: workspace, access: "read" },
    });
    const state = new NodeState(join(directory, "node.sqlite"));
    state.admit(command("00000000-0000-0000-0000-000000000023", "read link.txt"));
    const execution = state.claimNext();
    assert.ok(execution);

    await harness.execute(execution, state, new AbortController().signal);

    const finished = state
      .pendingPublications()[0]?.events.map((event) => event.kind)
      .find((kind) => kind.type === "tool_finished");
    assert.deepEqual(finished, {
      type: "tool_finished",
      call_id: "read-3",
      output: "path escapes the bound workspace",
      is_error: true,
    });
    state.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("the local write tool rejects a symlinked directory outside the workspace", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-write-symlink-"));
  const workspace = join(directory, "workspace");
  const outside = join(directory, "outside");
  try {
    await mkdir(workspace);
    await mkdir(outside);
    await writeFile(join(outside, "owned.txt"), "outside stays unchanged\n");
    await symlink(outside, join(workspace, "link"));
    const faux = fauxProvider();
    faux.setResponses([
      fauxAssistantMessage(
        fauxToolCall(
          "write",
          { path: "link/owned.txt", content: "escaped\n" },
          { id: "write-2" },
        ),
        { stopReason: "toolUse" },
      ),
      (context) => {
        const result = context.messages.findLast((message) => message.role === "toolResult");
        assert.ok(result);
        assert.equal(result.isError, true);
        assert.equal(contentText(result.content), "path escapes the bound workspace");
        return fauxAssistantMessage("The path is outside the authorized workspace.");
      },
    ]);
    const harness = new PiHarness({
      instructions: "Write carefully.",
      model: faux.getModel(),
      streamFn: faux.provider.streamSimple.bind(faux.provider),
      target: "workspace:renoa",
      workspace: { root: workspace, access: "read_write" },
    });
    const state = new NodeState(join(directory, "node.sqlite"));
    state.admit(command("00000000-0000-0000-0000-000000000026", "write link/owned.txt"));
    const execution = state.claimNext();
    assert.ok(execution);

    await harness.execute(execution, state, new AbortController().signal);

    const finished = state
      .pendingPublications()[0]?.events.map((event) => event.kind)
      .find((kind) => kind.type === "tool_finished");
    assert.deepEqual(finished, {
      type: "tool_finished",
      call_id: "write-2",
      output: "path escapes the bound workspace",
      is_error: true,
    });
    assert.equal(await readFile(join(outside, "owned.txt"), "utf8"), "outside stays unchanged\n");
    state.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("the Pi harness cannot cross its local target binding", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-target-"));
  try {
    const faux = fauxProvider();
    const harness = new PiHarness({
      instructions: "Answer clearly.",
      model: faux.getModel(),
      streamFn: () => {
        throw new Error("Pi must not run for a mismatched target");
      },
      target: "workspace:other",
    });
    const state = new NodeState(join(directory, "node.sqlite"));
    state.admit(command("00000000-0000-0000-0000-000000000024", "continue"));
    const execution = state.claimNext();
    assert.ok(execution);

    await harness.execute(execution, state, new AbortController().signal);

    assert.deepEqual(state.pendingPublications()[0]?.events.at(-1)?.kind, {
      type: "execution_terminated",
      terminal: {
        status: "failed",
        error: "Pi harness is not bound to the command target",
      },
    });
    state.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

function command(commandId: string, text: string): ExecuteCommand {
  return {
    taskId: "00000000-0000-0000-0000-000000000001",
    commandId,
    principalId: "00000000-0000-0000-0000-000000000003",
    surface: "test",
    target: "workspace:renoa",
    text,
  };
}
