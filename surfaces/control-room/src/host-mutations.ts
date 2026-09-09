import { reviewRepository } from "./host-contract";

export interface PendingChange { operation_id: string; expected_revision: number; [field: string]: unknown }
export interface ChangeResult { kind: "saved" | "rejected" | "uncertain"; message: string }
const key = (host: string, path: string) => `renoa:owner-change:v1:${host}:${path}`;

/** Persist only configuration edits, never cookies, credentials, or agent prompts.
 * A lost response can survive navigation/reload and retry the identical operation.
 */
export function pendingChange(host: string, path: string, storage: Storage = localStorage): PendingChange | null {
  const raw = storage.getItem(key(host, path));
  if (raw === null) return null;
  const value: unknown = JSON.parse(raw);
  if (!value || typeof value !== "object" || !("operation_id" in value) || typeof value.operation_id !== "string" ||
    !("expected_revision" in value) || !Number.isSafeInteger(value.expected_revision)) {
    throw new Error("The saved change cannot be read. Browser storage needs attention before another edit.");
  }
  return value as PendingChange;
}

export async function saveChange(host: string, path: string, fields: Record<string, unknown>,
  transport: typeof fetch = fetch, storage: Storage = localStorage): Promise<ChangeResult> {
  const request = pendingChange(host, path, storage) ?? { ...fields, operation_id: crypto.randomUUID() };
  // If browser storage fails, do not send a request whose identity may be lost.
  storage.setItem(key(host, path), JSON.stringify(request));
  function clearOwnReceipt() {
    if (pendingChange(host, path, storage)?.operation_id === request.operation_id) storage.removeItem(key(host, path));
  }
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 20_000);
  try {
    const response = await transport(path, { method: "POST", credentials: "same-origin", cache: "no-store",
      headers: { "Content-Type": "application/json" }, body: JSON.stringify(request), signal: controller.signal });
    if ([400, 404, 409, 413, 415, 422].includes(response.status)) {
      clearOwnReceipt();
      return { kind: "rejected", message: response.status === 409
        ? "Changed elsewhere. Refresh and review the current settings before trying again."
        : response.status === 422 ? "This change is not valid. An expired one-time schedule needs a new future date."
        : "The Host rejected this change. Refresh to check that the record still exists." };
    }
    if (!response.ok) return { kind: "uncertain", message: response.status === 401 || response.status === 403
      ? "Sign in as the Host owner, then retry this saved change."
      : "The Host could not confirm this change. Retry uses the same operation." };
    const body = await response.json();
    const valid = body?.operation_id === request.operation_id && (path.endsWith("/policy")
      ? reviewRepository(body.record) && String(body.record.policy.repository_id) === path.split("/").at(-2)
      : body.id === path.split("/").at(-2) && Number.isSafeInteger(body.revision) && typeof body.enabled === "boolean" && Number.isSafeInteger(body.next_due_ms));
    if (!valid) throw new Error("Unrecognized receipt");
    clearOwnReceipt();
    return { kind: "saved", message: "Saved by the Host. Refreshing current settings…" };
  } catch {
    return { kind: "uncertain", message: "The response was interrupted. The change may have saved; retry to recover its receipt." };
  } finally { clearTimeout(timeout); }
}
