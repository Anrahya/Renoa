import { lstat, realpath } from "node:fs/promises";
import { basename, dirname, isAbsolute, relative, resolve, sep } from "node:path";

import {
  createEditTool,
  createReadTool,
  createWriteTool,
  err,
  FileError,
  ok,
  type AgentTool,
  type AgentToolUpdateCallback,
  type EditToolDetails,
  type EditToolInput,
  type ReadToolDetails,
  type ReadToolInput,
  type Result,
  type WriteToolInput,
} from "@earendil-works/pi-agent-core";
import { NodeExecutionEnv } from "@earendil-works/pi-agent-core/node";

export type WorkspaceAccess = "read" | "read_write";

export interface WorkspaceConfig {
  readonly root: string;
  readonly access: WorkspaceAccess;
}

type ReadSchema = ReturnType<typeof createReadTool>["parameters"];
type EditSchema = ReturnType<typeof createEditTool>["parameters"];
type WriteSchema = ReturnType<typeof createWriteTool>["parameters"];

export function createWorkspaceTools(config: WorkspaceConfig): AgentTool[] {
  const environment = new WorkspaceExecutionEnv(resolve(config.root));
  const tools: AgentTool[] = [createWorkspaceReadTool(environment)];
  if (config.access === "read_write") {
    tools.push(createWorkspaceWriteTool(environment), createWorkspaceEditTool(environment));
  }
  return tools;
}

function createWorkspaceReadTool(
  environment: WorkspaceExecutionEnv,
): AgentTool<ReadSchema, ReadToolDetails | undefined> {
  const read = createReadTool();
  return {
    ...read,
    async execute(
      toolCallId: string,
      parameters: ReadToolInput,
      signal?: AbortSignal,
      onUpdate?: AgentToolUpdateCallback<ReadToolDetails | undefined>,
    ) {
      return read.execute(
        toolCallId,
        parameters,
        signal,
        onUpdate,
        { env: environment },
      );
    },
  };
}

function createWorkspaceWriteTool(
  environment: WorkspaceExecutionEnv,
): AgentTool<WriteSchema, undefined> {
  const write = createWriteTool();
  return {
    ...write,
    async execute(
      toolCallId: string,
      parameters: WriteToolInput,
      signal?: AbortSignal,
      onUpdate?: AgentToolUpdateCallback<undefined>,
    ) {
      return write.execute(
        toolCallId,
        parameters,
        signal,
        onUpdate,
        { env: environment },
      );
    },
  };
}

function createWorkspaceEditTool(
  environment: WorkspaceExecutionEnv,
): AgentTool<EditSchema, EditToolDetails | undefined> {
  const edit = createEditTool();
  return {
    ...edit,
    async execute(
      toolCallId: string,
      parameters: EditToolInput,
      signal?: AbortSignal,
      onUpdate?: AgentToolUpdateCallback<EditToolDetails | undefined>,
    ) {
      return edit.execute(
        toolCallId,
        parameters,
        signal,
        onUpdate,
        { env: environment },
      );
    },
  };
}

class WorkspaceExecutionEnv extends NodeExecutionEnv {
  readonly #root: string;

  constructor(root: string) {
    super({ cwd: root });
    this.#root = root;
  }

  override async absolutePath(path: string): Promise<Result<string, FileError>> {
    try {
      return ok(await confinedPath(this.#root, path));
    } catch (error) {
      if (error instanceof WorkspaceBoundaryError) {
        return err(new FileError("permission_denied", error.message, path, error));
      }
      const cause = error instanceof Error ? error : new Error(String(error));
      const code = isMissing(error) ? "not_found" : "unknown";
      return err(new FileError(code, cause.message, path, cause));
    }
  }
}

async function confinedPath(root: string, requested: string): Promise<string> {
  const canonicalRoot = await realpath(root);
  const addressed = resolve(root, requested);
  if (!isWithin(root, addressed) && !isWithin(canonicalRoot, addressed)) {
    throw new WorkspaceBoundaryError();
  }
  const missing: string[] = [];
  let cursor = addressed;
  for (;;) {
    try {
      await lstat(cursor);
    } catch (error) {
      if (!isMissing(error) || cursor === root) {
        throw error;
      }
      missing.unshift(basename(cursor));
      cursor = dirname(cursor);
      continue;
    }
    const canonical = await realpath(cursor);
    requireWithin(canonicalRoot, canonical);
    const result = resolve(canonical, ...missing);
    requireWithin(canonicalRoot, result);
    return result;
  }
}

function requireWithin(root: string, candidate: string): void {
  if (!isWithin(root, candidate)) {
    throw new WorkspaceBoundaryError();
  }
}

function isWithin(root: string, candidate: string): boolean {
  const path = relative(root, candidate);
  return path !== ".." && !path.startsWith(`..${sep}`) && !isAbsolute(path);
}

function isMissing(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === "ENOENT";
}

class WorkspaceBoundaryError extends Error {
  constructor() {
    super("path escapes the bound workspace");
  }
}
