/**
 * Adapted from Pi packages/ai
 * https://github.com/earendil-works/pi
 * Source revision: 914cf1472e715297caa30db4b9535d534a9eb718 (v0.84.2)
 * License: MIT
 * Copyright (c) 2025 Mario Zechner
 */

import OpenAI from "openai";
import type { Context, Model, ProviderHeaders } from "./types.js";
import { getCompat, type ResolvedOpenAICompletionsCompat } from "./openai-chat-compat.js";

/** Renoa retry wraps at a higher layer; transports send each request once. */
export async function retryProviderRequest<T>(
	request: () => Promise<T>,
	_options?: { maxRetries?: number; maxRetryDelayMs?: number; signal?: AbortSignal },
): Promise<T> {
	return await request();
}

export function hasHeader(headers: ProviderHeaders | undefined, name: string): boolean {
	if (!headers) return false;
	const expected = name.toLowerCase();
	for (const [key, value] of Object.entries(headers)) {
		if (key.toLowerCase() === expected && value !== null && value.trim().length > 0) return true;
	}
	return false;
}

export function getClientApiKey(provider: string, apiKey: string | undefined, headers: ProviderHeaders | undefined): string {
	if (apiKey) return apiKey;
	if (hasHeader(headers, "authorization") || hasHeader(headers, "cf-aig-authorization")) return "unused";
	throw new Error(`No API key for provider: ${provider}`);
}

export function createClient(
	model: Model<"openai-completions">,
	_context: Context,
	apiKey: string,
	optionsHeaders?: ProviderHeaders,
	fetch?: typeof globalThis.fetch,
	sessionId?: string,
	compat: ResolvedOpenAICompletionsCompat = getCompat(model),
) {
	const headers: ProviderHeaders = { ...model.headers };

	if (sessionId && compat.sendSessionAffinityHeaders) {
		if (compat.sessionAffinityFormat === "openai") {
			headers.session_id = sessionId;
		}
		headers["x-client-request-id"] = sessionId;
		headers["x-session-affinity"] = sessionId;
	}

	// Merge options headers last so they can override defaults
	if (optionsHeaders) {
		Object.assign(headers, optionsHeaders);
	}

	return new OpenAI({
		apiKey,
		baseURL: model.baseUrl,
		dangerouslyAllowBrowser: true,
		fetch,
		maxRetries: 0,
		defaultHeaders: headers,
	});
}
