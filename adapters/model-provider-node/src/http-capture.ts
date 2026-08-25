import type { AssistantMessage } from "./upstream/types.js";
import { isContextOverflow } from "./upstream/overflow.js";

export interface HttpCapture {
  dispatched?: boolean;
  status?: number;
  headers?: Record<string, string>;
  cause?: unknown;
}

export function capturingFetch(inner: typeof fetch, capture: HttpCapture): typeof fetch {
  return async (input, init) => {
    capture.dispatched = true;
    try {
      const response = await inner(input, init);
      capture.status = response.status;
      capture.headers = Object.fromEntries(response.headers.entries());
      return response;
    } catch (error) {
      capture.cause = error;
      throw error;
    }
  };
}

export function errorFromAssistant(
  message: AssistantMessage,
  contextWindow: number,
  capture: HttpCapture,
): Error {
  const overflow =
    message.stopReason === "error" &&
    message.usage.output === 0 &&
    message.content.every((content) => content.type === "text" && content.text.length === 0) &&
    isContextOverflow(message, contextWindow);
  const error = new Error(message.errorMessage ?? `model stopped with ${message.stopReason}`);
  if (overflow) {
    error.message = message.errorMessage ?? "context window exceeded";
  }
  if (capture.status !== undefined) {
    (error as Error & { status: number }).status = capture.status;
  }
  if (capture.headers !== undefined) {
    (error as Error & { headers: Record<string, string> }).headers = capture.headers;
  }
  if (capture.cause !== undefined) {
    (error as Error & { cause: unknown }).cause = capture.cause;
  }
  return error;
}

export function abortError(): Error {
  const error = new Error("The operation was aborted");
  error.name = "AbortError";
  return error;
}

export function isAbortError(error: unknown): boolean {
  if (typeof error !== "object" || error === null) {
    return false;
  }
  const record = error as { name?: unknown; message?: unknown; code?: unknown };
  return (
    record.name === "AbortError" ||
    record.code === "ABORT_ERR" ||
    record.message === "The operation was aborted" ||
    (typeof record.message === "string" && /request was aborted/i.test(record.message))
  );
}
