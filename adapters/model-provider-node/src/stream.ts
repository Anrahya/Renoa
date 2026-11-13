import type {
  JsonValue,
  WireErrorDiagnostic,
  WireModelRequest,
  WireStreamRecord,
} from "./contract.js";
import { ProviderFailure, classifyError, redactHeaders, redactSecrets } from "./errors.js";
import {
  MAX_ATTEMPTS,
  MAX_RATE_LIMIT_ATTEMPTS,
  MAX_RETRY_WAIT_MS,
  delayForAttempt,
  shouldRetry,
  systemClock,
  systemRandom,
  waitForRetry,
  type RetryClock,
  type RetryRandom,
} from "./retry.js";
import {
  dispatchStream,
  refreshOauth,
  resolveApiKey,
  type LoadedRuntime,
} from "./runtime.js";
import { capturingFetch, errorFromAssistant, abortError, isAbortError, type HttpCapture } from "./http-capture.js";
import { routedVertexModel } from "./providers/google-vertex.js";
import { parseWireModelRequest, toContext } from "./wire-request.js";
import { fromAssistant } from "./wire-response.js";
import type { AssistantMessageEvent, ClientIdentity, Context } from "@earendil-works/pi-ai";

const RENOA_IDENTITY: ClientIdentity = {
  userAgent: { custom: "renoa/0.1.0" },
  attribution: null,
};

export interface StreamInvocation {
  readonly runtime: LoadedRuntime;
  readonly request: WireModelRequest;
  readonly maxOutputTokens: number;
  readonly signal: AbortSignal;
  readonly emit: (record: WireStreamRecord) => void | Promise<void>;
  readonly clock?: RetryClock;
  readonly random?: RetryRandom;
  readonly fetch?: typeof fetch;
  readonly sessionId?: string;
}

export async function streamModel(invocation: StreamInvocation): Promise<void> {
  parseWireModelRequest(invocation.request);
  const clock = invocation.clock ?? systemClock;
  const random = invocation.random ?? systemRandom;
  let oauthRefreshed = false;
  let attempt = 0;
  let retryWaitMs = 0;
  while (attempt < MAX_RATE_LIMIT_ATTEMPTS) {
    attempt += 1;
    const attemptSignal = AbortSignal.any([invocation.signal]);
    const observed = { outputExposed: false };
    const capture: HttpCapture = {};
    try {
      await runAttempt(invocation, attemptSignal, observed, capture);
      return;
    } catch (error) {
      const cancelled = invocation.signal.aborted || isAbortError(error);
      const facts = classifyError(error, {
        outputExposed: observed.outputExposed,
        cancelled,
        dispatched: capture.dispatched === true,
      });
      if (
        facts.expiredOAuth &&
        !oauthRefreshed &&
        !observed.outputExposed &&
        !cancelled &&
        attempt < MAX_ATTEMPTS
      ) {
        oauthRefreshed = true;
        const credential = invocation.runtime.credentials.read(invocation.runtime.provider);
        if (credential?.type === "oauth") {
          try {
            await refreshOauth(invocation.runtime, credential, invocation.signal);
            continue;
          } catch (refreshError) {
            const refreshFacts = classifyError(refreshError, {
              outputExposed: false,
              cancelled: invocation.signal.aborted,
              dispatched: false,
            });
            throw new ProviderFailure(
              {
                ...refreshFacts,
                category: "authentication",
                retryable: false,
                expiredOAuth: true,
                inferenceOutcome: "known_not_started",
              },
              {
                provider: invocation.runtime.provider,
                model: invocation.runtime.model.id,
                attemptCount: attempt,
              },
            );
          }
        }
      }
      const delay = delayForAttempt(attempt, facts, random, clock.now());
      if (!shouldRetry(facts, attempt, observed.outputExposed) || cancelled || delay > MAX_RETRY_WAIT_MS - retryWaitMs) {
        throw new ProviderFailure(facts, {
          provider: invocation.runtime.provider,
          model: invocation.runtime.model.id,
          attemptCount: attempt,
        });
      }
      retryWaitMs += delay;
      await invocation.emit({
        event: "retry_attempt",
        attempt,
        next_attempt: attempt + 1,
        category: facts.category,
        delay_ms: delay,
        ...(facts.causeCode === undefined ? {} : { cause_code: facts.causeCode }),
      });
      try {
        await waitForRetry(delay, invocation.signal, clock);
      } catch (waitError) {
        throw new ProviderFailure(
          classifyError(waitError, {
            outputExposed: observed.outputExposed,
            cancelled: invocation.signal.aborted || isAbortError(waitError),
            dispatched:
              observed.outputExposed ||
              (capture.dispatched === true && facts.inferenceOutcome === "unknown"),
          }),
          {
            provider: invocation.runtime.provider,
            model: invocation.runtime.model.id,
            attemptCount: attempt,
          },
        );
      }
    }
  }
}

export function wireError(error: unknown, fallback: { provider: string; model: string }): WireStreamRecord {
  if (error instanceof ProviderFailure) {
    return {
      event: "error",
      error: error.message,
      error_kind: error.category,
      inference_outcome: error.inferenceOutcome,
      diagnostic: diagnosticFromFailure(error),
    };
  }
  const facts = classifyError(error, {
    outputExposed: false,
    cancelled: isAbortError(error),
    dispatched: false,
  });
  const failure = new ProviderFailure(facts, {
    provider: fallback.provider,
    model: fallback.model,
    attemptCount: 1,
  });
  return {
    event: "error",
    error: failure.message,
    error_kind: failure.category,
    inference_outcome: failure.inferenceOutcome,
    diagnostic: diagnosticFromFailure(failure),
  };
}

async function runAttempt(
  invocation: StreamInvocation,
  signal: AbortSignal,
  observed: { outputExposed: boolean },
  capture: HttpCapture,
): Promise<void> {
  if (invocation.runtime.provider === "opencode-go" &&
      (invocation.sessionId === undefined || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(invocation.sessionId))) {
    throw Object.assign(new Error("OpenCode Go requires RENOA_MODEL_SESSION_ID: a stable canonical UUID per conversation"), {
      categoryHint: "invalid_request",
    });
  }
  const apiKey = await resolveApiKey(invocation.runtime, signal);
  // Pi's Vertex transport sends through Google's SDK and refuses a custom
  // fetch, so Renoa cannot observe whether a Vertex request left the process:
  // every Vertex failure is treated as possibly dispatched.
  const vertex = invocation.runtime.model.api === "google-vertex";
  const fetchImpl = vertex ? undefined : capturingFetch(invocation.fetch ?? globalThis.fetch, capture);
  const model = vertex
    ? routedVertexModel(invocation.runtime.model, requireStoredCredential(invocation.runtime))
    : invocation.runtime.model;
  const options = {
    maxTokens: invocation.maxOutputTokens,
    // The Host owns the same total invocation deadline, including retries.
    // Avoid a shorter SDK default interrupting quiet, extended reasoning.
    timeoutMs: 30 * 60 * 1000,
    apiKey,
    signal,
    maxRetries: 0,
    ...(fetchImpl === undefined ? {} : { fetch: fetchImpl }),
    ...(invocation.sessionId === undefined ? {} : { sessionId: invocation.sessionId }),
    // Every provider sees Renoa, never Pi, and no attribution headers.
    identity: RENOA_IDENTITY,
    ...(invocation.runtime.provider === "opencode-go" && invocation.sessionId !== undefined
      ? { headers: { "x-opencode-session": invocation.sessionId } }
      : {}),
    onPayload: async (payload: unknown) => {
      await invocation.emit({
        event: "provider_request",
        payload: diagnosticValue(payload),
      });
      return undefined;
    },
    onResponse: async (response: { status: number; headers: Readonly<Record<string, string>> }) => {
      await invocation.emit({
        event: "provider_response",
        status: response.status,
        headers: redactHeaders(response.headers),
      });
    },
    ...(invocation.runtime.reasoningLevel === "off"
      ? {}
      : { reasoning: invocation.runtime.reasoningLevel }),
  };
  const thinkingSummaryQueue = new ResponsesThinkingSummaryTracker(invocation.runtime.model.api);
  const optionsWithTracker = {
    ...options,
    onProviderStreamEvent: (providerEvent: unknown) => {
      thinkingSummaryQueue.record(providerEvent);
    },
  };
  const context = withReplayedEffort(toContext(invocation.request), invocation.runtime);
  if (vertex) {
    capture.dispatched = true;
  }
  const stream = dispatchStream(model, context, optionsWithTracker);
  for await (const event of stream) {
    if (signal.aborted) {
      throw abortError();
    }
    const record = contentDelta(event, invocation.runtime.model, thinkingSummaryQueue);
    if (record !== undefined) {
      observed.outputExposed = true;
      await invocation.emit(record);
      continue;
    }
    if (event.type === "done") {
      await invocation.emit({
        event: "completed",
        response: fromAssistant(event.message, invocation.runtime.model.contextWindow),
      });
      return;
    }
    if (event.type === "error") {
      throw errorFromAssistant(event.error, invocation.runtime.model.contextWindow, capture);
    }
  }
  throw new Error("model stream closed without a terminal event");
}

function isSummarizedThinkingDelta(
  model: LoadedRuntime["model"],
  responsesSummaryTracker: ResponsesThinkingSummaryTracker,
): boolean {
  switch (model.api) {
    case "google-vertex":
      return true;
    case "anthropic-messages":
      return (
        model.provider === "anthropic" &&
        model.id.startsWith("claude-") &&
        !model.id.startsWith("claude-3-7")
      );
    case "openai-responses":
      return responsesSummaryTracker.consumeDeltaIsSummary();
    default:
      return false;
  }
}

class ResponsesThinkingSummaryTracker {
  readonly #enabled: boolean;
  readonly #reasoningSlots = new Set<number>();
  readonly #pending: boolean[] = [];

  constructor(api: string) {
    this.#enabled = api === "openai-responses";
  }

  record(event: unknown): void {
    if (!this.#enabled || typeof event !== "object" || event === null) {
      return;
    }
    const candidate = event as {
      readonly type?: unknown;
      readonly output_index?: unknown;
      readonly item?: { readonly type?: unknown } | null;
    };
    const index = candidate.output_index;
    if (typeof index !== "number") {
      return;
    }
    if (candidate.type === "response.output_item.added") {
      if (candidate.item?.type === "reasoning") {
        this.#reasoningSlots.add(index);
      } else {
        this.#reasoningSlots.delete(index);
      }
      return;
    }
    if (candidate.type === "response.output_item.done") {
      this.#reasoningSlots.delete(index);
      return;
    }
    if (!this.#reasoningSlots.has(index)) {
      return;
    }
    if (
      candidate.type === "response.reasoning_summary_text.delta" ||
      candidate.type === "response.reasoning_summary_part.done"
    ) {
      this.#pending.push(true);
    } else if (candidate.type === "response.reasoning_text.delta") {
      this.#pending.push(false);
    }
  }

  consumeDeltaIsSummary(): boolean {
    return this.#pending.shift() === true;
  }
}

function contentDelta(
  event: AssistantMessageEvent,
  model: LoadedRuntime["model"],
  responsesSummaryTracker: ResponsesThinkingSummaryTracker,
): WireStreamRecord | undefined {
  switch (event.type) {
    case "text_delta":
      return {
        event: "content_delta",
        content_index: event.contentIndex,
        delta: { type: "text", text: event.delta },
      };
    case "thinking_delta":
      return {
        event: "content_delta",
        content_index: event.contentIndex,
        delta: {
          type: "reasoning",
          text: event.delta,
          ...(isSummarizedThinkingDelta(model, responsesSummaryTracker)
            ? { summarized: true }
            : {}),
        },
      };
    case "toolcall_start": {
      const block = event.partial.content[event.contentIndex];
      if (block?.type !== "toolCall") {
        throw new Error("tool-call start is missing its partial tool call");
      }
      return {
        event: "content_delta",
        content_index: event.contentIndex,
        delta: { type: "tool_call_start", id: block.id, name: block.name },
      };
    }
    case "toolcall_delta":
      return {
        event: "content_delta",
        content_index: event.contentIndex,
        delta: { type: "tool_call_arguments", json_delta: event.delta },
      };
    case "start":
    case "text_start":
    case "text_end":
    case "thinking_start":
    case "thinking_end":
    case "toolcall_end":
    case "done":
    case "error":
      return undefined;
  }
}

function diagnosticValue(value: unknown): JsonValue {
  const encoded = JSON.stringify(redactSecrets(value));
  if (encoded === undefined) {
    return null;
  }
  return JSON.parse(encoded) as JsonValue;
}

function diagnosticFromFailure(error: ProviderFailure): WireErrorDiagnostic {
  return {
    provider: error.provider,
    model: error.model,
    attempt_count: error.attemptCount,
    ...(error.httpStatus === undefined ? {} : { http_status: error.httpStatus }),
    ...(error.providerCode === undefined ? {} : { provider_code: error.providerCode }),
    ...(error.requestId === undefined ? {} : { request_id: error.requestId }),
    ...(error.retryAfter === undefined ? {} : { retry_after: error.retryAfter }),
    ...(error.causeCode === undefined ? {} : { cause_code: error.causeCode }),
    ...(error.causeMessage === undefined ? {} : { cause_message: error.causeMessage }),
    ...(error.providerMessage === undefined ? {} : { provider_message: error.providerMessage }),
  };
}

const ANTHROPIC_EFFORTS: ReadonlySet<string> = new Set(["low", "medium", "high", "xhigh", "max"]);

/**
 * Pi sets an Anthropic model's effort with a system marker after the last
 * message, and on replay puts each reply's own level back before it, so the
 * history repeats what was sent and its thinking blocks stay valid. The wire
 * does not carry a reply's level; a session's replies were made at its current
 * level unless the owner changed it, and then Anthropic drops the earlier
 * blocks (`drop_block`) rather than refusing the turn.
 */
export function withReplayedEffort(context: Context, runtime: LoadedRuntime): Context {
  const model = runtime.model;
  const level = runtime.reasoningLevel;
  if (model.api !== "anthropic-messages" || !ANTHROPIC_EFFORTS.has(level)) {
    return context;
  }
  return {
    ...context,
    messages: context.messages.map((message) =>
      message.role === "assistant" &&
      message.api === model.api &&
      message.provider === model.provider &&
      message.providerThinkingLevel === undefined
        ? { ...message, providerThinkingLevel: level }
        : message,
    ),
  };
}

function requireStoredCredential(runtime: LoadedRuntime) {
  const credential = runtime.credentials.read(runtime.provider);
  if (credential === undefined) {
    throw new Error(`${runtime.provider} credentials are not configured`);
  }
  return credential;
}
