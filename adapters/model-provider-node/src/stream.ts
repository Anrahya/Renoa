import type {
  JsonValue,
  WireErrorDiagnostic,
  WireModelRequest,
  WireStreamRecord,
} from "./contract.js";
import { ProviderFailure, classifyError, redactHeaders, redactSecrets } from "./errors.js";
import {
  MAX_ATTEMPTS,
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
import { fromAssistant, parseWireModelRequest, toContext } from "./wire-request.js";
import type { AssistantMessageEvent } from "./upstream/types.js";

export interface StreamInvocation {
  readonly runtime: LoadedRuntime;
  readonly request: WireModelRequest;
  readonly maxOutputTokens: number;
  readonly signal: AbortSignal;
  readonly emit: (record: WireStreamRecord) => void | Promise<void>;
  readonly clock?: RetryClock;
  readonly random?: RetryRandom;
  readonly fetch?: typeof fetch;
}

export async function streamModel(invocation: StreamInvocation): Promise<void> {
  parseWireModelRequest(invocation.request);
  const clock = invocation.clock ?? systemClock;
  const random = invocation.random ?? systemRandom;
  let oauthRefreshed = false;
  let attempt = 0;
  while (attempt < MAX_ATTEMPTS) {
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
      if (!shouldRetry(facts, attempt, observed.outputExposed) || cancelled) {
        throw new ProviderFailure(facts, {
          provider: invocation.runtime.provider,
          model: invocation.runtime.model.id,
          attemptCount: attempt,
        });
      }
      const delay = delayForAttempt(facts.retryAfter === undefined ? attempt : 1, facts, random, clock.now());
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
  const apiKey = await resolveApiKey(invocation.runtime, signal);
  const fetchImpl = capturingFetch(invocation.fetch ?? globalThis.fetch, capture);
  const options = {
    maxTokens: invocation.maxOutputTokens,
    apiKey,
    signal,
    maxRetries: 0,
    fetch: fetchImpl,
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
  const stream = dispatchStream(invocation.runtime.model, toContext(invocation.request), options);
  for await (const event of stream) {
    if (signal.aborted) {
      throw abortError();
    }
    const record = contentDelta(event);
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

function contentDelta(event: AssistantMessageEvent): WireStreamRecord | undefined {
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
        delta: { type: "reasoning", text: event.delta },
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
