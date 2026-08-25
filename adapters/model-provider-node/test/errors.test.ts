import assert from "node:assert/strict";
import { test } from "node:test";

import { classifyError, ProviderFailure, redactSecrets, redactText } from "../src/errors.js";
import { delayForAttempt, parseRetryAfter, shouldRetry, MAX_ATTEMPTS } from "../src/retry.js";

test("connection reset before HTTP is unknown once the request may have been transmitted", () => {
  const error = Object.assign(new Error("APIConnectionError: Connection error."), {
    name: "APIConnectionError",
    cause: Object.assign(new Error("read ECONNRESET"), { code: "ECONNRESET" }),
  });
  const beforeDispatch = classifyError(error, { outputExposed: false, cancelled: false, dispatched: false });
  assert.equal(beforeDispatch.inferenceOutcome, "known_not_started");
  assert.equal(beforeDispatch.retryable, true);
  const facts = classifyError(error, { outputExposed: false, cancelled: false, dispatched: true });
  assert.equal(facts.category, "network");
  assert.equal(facts.causeCode, "ECONNRESET");
  assert.match(facts.causeMessage ?? "", /ECONNRESET/);
  assert.equal(facts.httpStatus, undefined);
  assert.equal(facts.retryable, true);
  assert.equal(facts.inferenceOutcome, "unknown");
  const failure = new ProviderFailure(facts, { provider: "xai", model: "grok-4.6", attemptCount: 3 });
  assert.equal(
    failure.message,
    "xAI request failed after 3 attempts: connection reset after the request may have been transmitted (ECONNRESET).",
  );
  const preDispatchFailure = new ProviderFailure(beforeDispatch, {
    provider: "xai",
    model: "grok-4.6",
    attemptCount: 3,
  });
  assert.equal(
    preDispatchFailure.message,
    "xAI request failed after 3 attempts: connection reset before an HTTP response (ECONNRESET).",
  );
});

test("ordinary 400 is not retried and keeps provider body metadata", () => {
  const error = Object.assign(new Error("invalid_request_error: bad schema"), {
    status: 400,
    requestID: "req_400",
    headers: { "x-request-id": "req_400" },
    error: { code: "invalid_request_error", message: "bad schema" },
  });
  const facts = classifyError(error, { outputExposed: false, cancelled: false });
  assert.equal(facts.category, "invalid_request");
  assert.equal(facts.retryable, false);
  assert.equal(facts.httpStatus, 400);
  assert.equal(facts.requestId, "req_400");
  assert.equal(shouldRetry(facts, 1, false), false);
});

test("429 honors Retry-After seconds and remains retryable until the attempt budget", () => {
  const facts = classifyError(
    Object.assign(new Error("rate limited"), {
      status: 429,
      headers: { "retry-after": "2", "x-request-id": "req_429" },
    }),
    { outputExposed: false, cancelled: false },
  );
  assert.equal(facts.category, "rate_limited");
  assert.equal(facts.retryable, true);
  assert.equal(parseRetryAfter(facts.retryAfter, 0), 2_000);
  assert.equal(delayForAttempt(1, facts, { jitter: () => 0 }, 0), 2_000);
  assert.equal(shouldRetry(facts, 2, false), true);
  assert.equal(shouldRetry(facts, MAX_ATTEMPTS, false), false);
  const failure = new ProviderFailure(facts, { provider: "xai", model: "grok-4.6", attemptCount: 3 });
  assert.match(failure.message, /rate limited \(429\) \(request req_429\)/);
});

test("provider body is preserved in a bounded redacted diagnostic field", () => {
  const facts = classifyError(
    Object.assign(new Error('400: {"error":{"message":"max_tokens is too large for this model","type":"invalid_request_error"}}'), {
      status: 400,
    }),
    { outputExposed: false, cancelled: false, dispatched: true },
  );
  assert.equal(facts.category, "invalid_request");
  assert.equal(facts.inferenceOutcome, "known_not_started");
  assert.match(facts.providerMessage ?? "", /max_tokens is too large/);
  const failure = new ProviderFailure(facts, { provider: "xai", model: "grok-4.6", attemptCount: 1 });
  assert.match(failure.providerMessage ?? "", /max_tokens is too large/);
});

test("JSON syntax errors from a broken SSE payload are protocol failures", () => {
  const facts = classifyError(new SyntaxError("Expected property name or '}' in JSON at position 1"), {
    outputExposed: false,
    cancelled: false,
  });
  assert.equal(facts.category, "protocol");
  assert.equal(facts.inferenceOutcome, "unknown");
  assert.equal(facts.retryable, false);
});

test("output already exposed is never retried", () => {
  const facts = classifyError(
    Object.assign(new Error("read ECONNRESET"), { cause: { code: "ECONNRESET" } }),
    { outputExposed: true, cancelled: false },
  );
  assert.equal(facts.category, "stream_interrupted");
  assert.equal(facts.retryable, false);
  assert.equal(facts.inferenceOutcome, "unknown");
  assert.equal(shouldRetry(facts, 1, true), false);
});

test("textual provider errors redact nested secrets and keep telemetry", () => {
  const nested = {
    error: {
      message: "rejected",
      credentials: {
        access_token: "stolen-access",
        refresh_token: "stolen-refresh",
        api_key: "stolen-api-key",
      },
    },
    usage: { prompt_tokens: 12, completion_tokens: 4 },
    max_tokens: 128,
    x_ratelimit_remaining: "9",
  };
  const http = `400: ${JSON.stringify(nested)}`;
  const facts = classifyError(Object.assign(new Error(http), { status: 400 }), {
    outputExposed: false,
    cancelled: false,
    dispatched: true,
  });
  const combined = `${facts.providerMessage ?? ""} ${facts.causeMessage ?? ""} ${facts.rawMessage}`;
  assert.equal(combined.includes("stolen-access"), false);
  assert.equal(combined.includes("stolen-refresh"), false);
  assert.equal(combined.includes("stolen-api-key"), false);
  assert.match(facts.providerMessage ?? "", /max_tokens/);
  assert.match(facts.providerMessage ?? "", /prompt_tokens/);
  assert.match(facts.providerMessage ?? "", /x_ratelimit_remaining/);

  const headers = redactText(
    JSON.stringify({
      Cookie: "session=abc123",
      Authorization: "Basic dXNlcjpwYXNz",
      extra: "Bearer sk-live-nested",
    }),
  );
  const parsed = JSON.parse(headers) as { Cookie: string; Authorization: string; extra: string };
  assert.equal(parsed.Cookie, "<redacted>");
  assert.equal(parsed.Authorization, "<redacted>");
  assert.equal(parsed.extra.includes("sk-live-nested"), false);
  assert.match(parsed.extra, /Bearer <redacted>/);
  const httpHeaders = redactText("Cookie: session=abc123\nAuthorization: Basic dXNlcjpwYXNz");
  assert.equal(httpHeaders.includes("abc123"), false);
  assert.equal(httpHeaders.includes("dXNlcjpwYXNz"), false);
  assert.match(httpHeaders, /Cookie: <redacted>/);
  assert.match(httpHeaders, /Basic <redacted>/);

  const unknown = classifyError(new Error(`unexpected: ${JSON.stringify({ access_token: "leak-me" })}`), {
    outputExposed: false,
    cancelled: false,
  });
  const unknownFailure = new ProviderFailure(unknown, { provider: "xai", model: "grok-4.6", attemptCount: 1 });
  assert.equal(unknownFailure.message.includes("leak-me"), false);

  const caused = classifyError(
    Object.assign(new Error("provider failed"), {
      cause: { message: JSON.stringify({ refresh_token: "cause-secret", Cookie: "session=abc" }) },
    }),
    { outputExposed: false, cancelled: false },
  );
  assert.equal((caused.causeMessage ?? "").includes("cause-secret"), false);
  assert.equal((caused.causeMessage ?? "").includes("session=abc"), false);

  const common = redactText(
    [
      '{"client_secret":"cs-live","private_key":"pk-live"}',
      "refresh_token=rt-form-value&access_token=at-form",
      'client_secret="quoted-secret"',
      "client_secret = spaced-secret",
      String.raw`client_secret=\"escaped-secret\"`,
      "Authorization: ApiKey ak-live-123",
      "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC\n-----END PRIVATE KEY-----",
      "-----BEGIN RSA PRIVATE KEY-----\ntruncated-pem-body-leak",
    ].join("\n"),
  );
  assert.equal(common.includes("cs-live"), false);
  assert.equal(common.includes("pk-live"), false);
  assert.equal(common.includes("rt-form-value"), false);
  assert.equal(common.includes("at-form"), false);
  assert.equal(common.includes("ak-live-123"), false);
  assert.equal(common.includes("quoted-secret"), false);
  assert.equal(common.includes("spaced-secret"), false);
  assert.equal(common.includes("escaped-secret"), false);
  assert.equal(common.includes("MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC"), false);
  assert.equal(common.includes("truncated-pem-body-leak"), false);
  assert.match(common, /client_secret":"<redacted>"/);
  assert.match(common, /private_key":"<redacted>"/);
  assert.match(common, /refresh_token=<redacted>/);
  assert.match(common, /client_secret=<redacted>/);
  assert.match(common, /Authorization: ApiKey <redacted>/);
  assert.match(common, /<redacted-private-key>/);

  assert.equal(redactText("client_secret = spaced-secret").includes("spaced-secret"), false);
  assert.equal(
    redactText(String.raw`client_secret=\"escaped-secret\"`).includes("escaped-secret"),
    false,
  );
  assert.equal(
    redactText("-----BEGIN RSA PRIVATE KEY-----\ntruncated-pem-body-leak").includes(
      "truncated-pem-body-leak",
    ),
    false,
  );
  assert.equal(
    redactText(String.raw`wrapped: {\"client_secret\":\"nested-escaped-secret\"}`).includes(
      "nested-escaped-secret",
    ),
    false,
  );
  assert.equal(redactText("x-api-key: header-secret").includes("header-secret"), false);
  assert.equal(
    redactText("client_secret = unquoted two word secret").includes("two word secret"),
    false,
  );
  assert.equal(
    redactText("client_secret=<redacted> x-api-key=second-secret").includes("second-secret"),
    false,
  );
});

test("status-less looking errors are unknown after dispatch and outputExposed stays unknown", () => {
  const cases = [
    "invalid api key",
    "invalid_request_error: bad schema",
    "prompt is too long for the context window",
    "rate limited: too many requests",
  ];
  for (const message of cases) {
    const facts = classifyError(new Error(message), {
      outputExposed: false,
      cancelled: false,
      dispatched: true,
    });
    assert.equal(facts.inferenceOutcome, "unknown", message);
  }
  const rejected = classifyError(Object.assign(new Error("invalid request"), { status: 400 }), {
    outputExposed: false,
    cancelled: false,
    dispatched: true,
  });
  assert.equal(rejected.inferenceOutcome, "known_not_started");
  const exposed = classifyError(Object.assign(new Error("invalid request"), { status: 401 }), {
    outputExposed: true,
    cancelled: false,
    dispatched: true,
  });
  assert.equal(exposed.inferenceOutcome, "unknown");
});

test("redaction matches exact sensitive fields and preserves token telemetry", () => {
  const redacted = redactSecrets({
    authorization: "Bearer super-secret",
    cookie: "session=abc",
    token: "xyz",
    access_token: "stolen-access",
    max_tokens: 128,
    max_output_tokens: 256,
    input_tokens: 41,
    prompt_tokens: 12,
    nested: {
      cause: { message: "https://example.test/callback?token=stolen&sig=deadbeef&ok=1", retry_after: "2" },
      x_ratelimit_remaining: "10",
    },
  });
  const encoded = JSON.stringify(redacted);
  assert.equal(encoded.includes("super-secret"), false);
  assert.equal(encoded.includes("stolen-access"), false);
  assert.equal(encoded.includes("stolen"), false);
  assert.equal(encoded.includes("xyz"), false);
  assert.equal(encoded.includes("session=abc"), false);
  assert.match(encoded, /<redacted>/);
  assert.equal((redacted as { max_tokens: number }).max_tokens, 128);
  assert.equal((redacted as { max_output_tokens: number }).max_output_tokens, 256);
  assert.equal((redacted as { input_tokens: number }).input_tokens, 41);
  assert.equal((redacted as { prompt_tokens: number }).prompt_tokens, 12);
  assert.equal(
    (redacted as { nested: { x_ratelimit_remaining: string } }).nested.x_ratelimit_remaining,
    "10",
  );
  assert.equal(redactText("Authorization: Bearer sk-live-123").includes("sk-live-123"), false);
});
