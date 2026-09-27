export interface ModelOption { provider: string; model: string; name: string; reasoning_levels: string[]; default_reasoning: string | null }
export interface CreationOptions { native_tools: string[]; models: ModelOption[]; default_model: { provider: string; model: string; reasoning: string | null } }
export interface DiscordBinding { channel_id: string; agent_id: string; channel_name: string; revision: number }
export interface DiscordState { status: "setup_required" | "configured"; bindings: DiscordBinding[] }
type ObjectValue = Record<string, unknown>;
const object = (value: unknown): value is ObjectValue => typeof value === "object" && value !== null && !Array.isArray(value);
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(item => typeof item === "string");
const uuid = (value: unknown): value is string => typeof value === "string" && /^[\da-f]{8}-(?:[\da-f]{4}-){3}[\da-f]{12}$/i.test(value);
export function parseCreationOptions(value: unknown): CreationOptions {
  if (!object(value) || !strings(value.native_tools) || !object(value.default_model) || typeof value.default_model.provider !== "string" || typeof value.default_model.model !== "string" || !(value.default_model.reasoning === null || typeof value.default_model.reasoning === "string") || !Array.isArray(value.models) || !value.models.every(model => object(model) && typeof model.provider === "string" && typeof model.model === "string" && typeof model.name === "string" && strings(model.reasoning_levels) && (model.default_reasoning === null || typeof model.default_reasoning === "string" && model.reasoning_levels.includes(model.default_reasoning)))) throw new Error("The Host returned incompatible creation options.");
  return value as unknown as CreationOptions;
}
export function isBinding(value: unknown): value is DiscordBinding {
  return object(value) && typeof value.channel_id === "string" && /^[1-9]\d{0,19}$/.test(value.channel_id) && uuid(value.agent_id) && typeof value.channel_name === "string" && Number.isSafeInteger(value.revision) && (value.revision as number) > 0;
}
export function parseDiscord(value: unknown): DiscordState {
  if (!object(value) || !["setup_required", "configured"].includes(value.status as string) || !Array.isArray(value.bindings) || !value.bindings.every(isBinding) || value.status === "setup_required" && value.bindings.length !== 0) throw new Error("The Host returned incompatible Discord settings.");
  return value as unknown as DiscordState;
}

export async function readOwner<T>(path: string, parse: (value: unknown) => T, signal: AbortSignal): Promise<T> {
  const response = await fetch(path, { credentials: "same-origin", cache: "no-store", signal });
  if (!response.ok) { const error = await response.json().catch(() => null); throw new Error(error?.message ?? "The Host is unavailable. Retry when connected."); }
  return parse(await response.json());
}

const pendingKey = (host: string, path: string) => `renoa:onboarding:v1:${host}:${path}`;
export function pendingOperation(host: string, path: string, storage: Storage = sessionStorage): ObjectValue | null {
  const raw = storage.getItem(pendingKey(host, path));
  if (raw === null) return null;
  const value: unknown = JSON.parse(raw);
  if (!object(value) || !uuid(value.operation_id)) throw new Error("The pending request cannot be read. Restore this tab’s storage before retrying.");
  return value;
}
export function discardOperation(host: string, path: string) { sessionStorage.removeItem(pendingKey(host, path)); }
export type OwnerResult = { kind: "saved"; record: ObjectValue } | { kind: "rejected" | "uncertain"; message: string };
export async function writeOwner(host: string, path: string, fields: ObjectValue, valid: (value: unknown, request: ObjectValue) => boolean,
  transport: typeof fetch = fetch, storage: Storage = sessionStorage): Promise<OwnerResult> {
  const request = pendingOperation(host, path, storage) ?? { ...fields, operation_id: crypto.randomUUID() };
  storage.setItem(pendingKey(host, path), JSON.stringify(request));
  const clear = () => { if (pendingOperation(host, path, storage)?.operation_id === request.operation_id) storage.removeItem(pendingKey(host, path)); };
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await transport(path, { method: "POST", credentials: "same-origin", cache: "no-store", headers: { "Content-Type": "application/json" }, body: JSON.stringify(request), signal: controller.signal });
    const receipt: unknown = await response.json();
    if ([400, 404, 409, 413, 415, 422].includes(response.status)) {
      clear();
      return { kind: "rejected", message: object(receipt) && typeof receipt.message === "string" ? receipt.message : "The Host rejected this request. Review the settings before retrying." };
    }
    if (response.status === 401 || response.status === 403) return { kind: "uncertain", message: "Sign in as the Host owner, then retry this saved request." };
    if (!response.ok || !object(receipt) || receipt.operation_id !== request.operation_id || !object(receipt.record) || !valid(receipt.record, request)) throw new Error("Unconfirmed receipt");
    clear();
    return { kind: "saved", record: receipt.record };
  } catch { return { kind: "uncertain", message: "The result could not be confirmed. Retry the saved request to recover its outcome." }; }
  finally { clearTimeout(timeout); }
}

export const validCreatedAgent = (value: unknown): boolean => object(value) && uuid(value.id) && typeof value.name === "string" && object(value.operational) && typeof value.operational.instructions === "string" && object(value.tool_selection) && strings(value.tool_selection.tools) && Number.isSafeInteger(value.tool_selection.revision);
