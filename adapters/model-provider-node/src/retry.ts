import type { FailureFacts } from "./errors.js";

export const MAX_ATTEMPTS = 3;
export const MAX_RETRY_AFTER_MS = 60_000;
const BASE_DELAY_MS = 250;

export interface RetryClock {
  now(): number;
  sleep(ms: number, signal: AbortSignal): Promise<void>;
}

export interface RetryRandom {
  /** Returns a value in [0, 1). */
  jitter(): number;
}

export const systemClock: RetryClock = {
  now: () => Date.now(),
  sleep: abortableSleep,
};

export const systemRandom: RetryRandom = {
  jitter: () => Math.random(),
};

export function delayForAttempt(
  attempt: number,
  facts: FailureFacts,
  random: RetryRandom,
  nowMs: number,
): number {
  const retryAfter = parseRetryAfter(facts.retryAfter, nowMs);
  if (retryAfter !== undefined) {
    return Math.min(retryAfter, MAX_RETRY_AFTER_MS);
  }
  const exponential = BASE_DELAY_MS * 2 ** Math.max(0, attempt - 1);
  const jittered = exponential * (0.5 + random.jitter() * 0.5);
  return Math.min(Math.round(jittered), MAX_RETRY_AFTER_MS);
}

/** RFC 9110 Retry-After: delay-seconds or HTTP-date. */
export function parseRetryAfter(value: string | undefined, nowMs: number): number | undefined {
  if (value === undefined || value.length === 0) {
    return undefined;
  }
  const asNumber = Number(value);
  if (Number.isFinite(asNumber) && asNumber >= 0) {
    return asNumber * 1_000;
  }
  const date = Date.parse(value);
  if (Number.isNaN(date)) {
    return undefined;
  }
  return Math.max(0, date - nowMs);
}

export function shouldRetry(facts: FailureFacts, attempt: number, outputExposed: boolean): boolean {
  if (outputExposed || attempt >= MAX_ATTEMPTS) {
    return false;
  }
  return facts.retryable;
}

export async function waitForRetry(
  ms: number,
  signal: AbortSignal,
  clock: RetryClock,
): Promise<void> {
  if (ms <= 0) {
    signal.throwIfAborted();
    return;
  }
  await clock.sleep(ms, signal);
}

function abortableSleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) {
      reject(abortError());
      return;
    }
    const timeout = setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timeout);
      reject(abortError());
    };
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

function abortError(): Error {
  const error = new Error("The operation was aborted");
  error.name = "AbortError";
  return error;
}
