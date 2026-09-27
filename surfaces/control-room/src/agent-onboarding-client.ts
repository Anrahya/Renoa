export interface ModelOption { provider: string; model: string; name: string; reasoning_levels: string[]; default_reasoning: string | null }
export interface CreationOptions { native_tools: string[]; models: ModelOption[]; default_model: { provider: string; model: string; reasoning: string | null } }
export interface DiscordBinding { channel_id: string; agent_id: string; channel_name: string; revision: number }
export type DiscordState = { status: "setup_required" } | { status: "connected"; bot_name: string; guild_name: string; default_agent_id: string; bindings: DiscordBinding[] };
export interface DiscordGuild { id: string; name: string; administrator: boolean }
export interface DiscordInspection { bot_name: string; invite_url: string; guilds: DiscordGuild[] }
export interface DiscordChannel { id: string; name: string }
type ObjectValue = Record<string, unknown>;
const object = (value: unknown): value is ObjectValue => typeof value === "object" && value !== null && !Array.isArray(value);
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every(item => typeof item === "string");
const uuid = (value: unknown): value is string => typeof value === "string" && /^[\da-f]{8}-(?:[\da-f]{4}-){3}[\da-f]{12}$/i.test(value);
export function parseCreationOptions(value: unknown): CreationOptions {
  if (!object(value) || !strings(value.native_tools) || !object(value.default_model) || typeof value.default_model.provider !== "string" || typeof value.default_model.model !== "string" || !(value.default_model.reasoning === null || typeof value.default_model.reasoning === "string") || !Array.isArray(value.models) || !value.models.every(model => object(model) && typeof model.provider === "string" && typeof model.model === "string" && typeof model.name === "string" && strings(model.reasoning_levels) && (model.default_reasoning === null || typeof model.default_reasoning === "string" && model.reasoning_levels.includes(model.default_reasoning)))) throw new Error("The Host returned incompatible creation options.");
  return value as unknown as CreationOptions;
}
const snowflake = (value: unknown): value is string => typeof value === "string" && /^[1-9]\d{0,19}$/.test(value);
export function isBinding(value: unknown): value is DiscordBinding {
  return object(value) && snowflake(value.channel_id) && uuid(value.agent_id) && typeof value.channel_name === "string" && Number.isSafeInteger(value.revision) && (value.revision as number) > 0;
}
export function isDiscordState(value: unknown): value is DiscordState {
  if (!object(value)) return false;
  if (value.status === "setup_required") return Object.keys(value).length === 1;
  return value.status === "connected" && typeof value.bot_name === "string" && typeof value.guild_name === "string" && uuid(value.default_agent_id) && Array.isArray(value.bindings) && value.bindings.every(isBinding);
}
export function parseDiscord(value: unknown): DiscordState {
  if (!isDiscordState(value)) throw new Error("The Host returned incompatible Discord settings.");
  return value;
}
export function parseInspection(value: unknown): DiscordInspection {
  if (!object(value) || typeof value.bot_name !== "string" || typeof value.invite_url !== "string" || !value.invite_url.startsWith("https://discord.com/oauth2/authorize?") || !Array.isArray(value.guilds) || !value.guilds.every(guild => object(guild) && snowflake(guild.id) && typeof guild.name === "string" && typeof guild.administrator === "boolean")) throw new Error("The Host returned an incompatible Discord token check.");
  return value as unknown as DiscordInspection;
}
export function parseChannels(value: unknown): DiscordChannel[] {
  if (!Array.isArray(value) || !value.every(channel => object(channel) && snowflake(channel.id) && typeof channel.name === "string")) throw new Error("The Host returned incompatible Discord channels.");
  return value as DiscordChannel[];
}

export async function readOwner<T>(path: string, parse: (value: unknown) => T, signal: AbortSignal, body?: ObjectValue): Promise<T> {
  const init: RequestInit = body === undefined ? {} : { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) };
  const response = await fetch(path, { ...init, credentials: "same-origin", cache: "no-store", signal });
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
  const result = await submitOwner(path, request, valid, transport);
  if (result.kind !== "uncertain" && pendingOperation(host, path, storage)?.operation_id === request.operation_id) storage.removeItem(pendingKey(host, path));
  return result;
}

/** Sends one owner operation without retaining it; the caller keeps any retry state in memory. */
export async function submitOwner(path: string, request: ObjectValue, valid: (value: unknown, request: ObjectValue) => boolean, transport: typeof fetch = fetch): Promise<OwnerResult> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await transport(path, { method: "POST", credentials: "same-origin", cache: "no-store", headers: { "Content-Type": "application/json" }, body: JSON.stringify(request), signal: controller.signal });
    const receipt: unknown = await response.json();
    if ([400, 404, 409, 413, 415, 422].includes(response.status)) return { kind: "rejected", message: object(receipt) && typeof receipt.message === "string" ? receipt.message : "The Host rejected this request. Review the settings before retrying." };
    if (response.status === 401 || response.status === 403) return { kind: "uncertain", message: "Sign in as the Host owner, then retry this saved request." };
    if (!response.ok || !object(receipt) || receipt.operation_id !== request.operation_id || !object(receipt.record) || !valid(receipt.record, request)) throw new Error("Unconfirmed receipt");
    return { kind: "saved", record: receipt.record };
  } catch { return { kind: "uncertain", message: "The result could not be confirmed. Retry the saved request to recover its outcome." }; }
  finally { clearTimeout(timeout); }
}

export const validCreatedAgent = (value: unknown): boolean => object(value) && uuid(value.id) && typeof value.name === "string" && object(value.operational) && typeof value.operational.instructions === "string" && object(value.tool_selection) && strings(value.tool_selection.tools) && Number.isSafeInteger(value.tool_selection.revision);
