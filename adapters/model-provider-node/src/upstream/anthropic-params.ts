/**
 * Adapted from Pi packages/ai
 * https://github.com/earendil-works/pi
 * Source revision: 914cf1472e715297caa30db4b9535d534a9eb718 (v0.84.2)
 * License: MIT
 * Copyright (c) 2025 Mario Zechner
 */

import type {
	CacheControlEphemeral,
	MessageCreateParamsStreaming,
} from "@anthropic-ai/sdk/resources/messages.js";
import type {
	AnthropicMessagesCompat,
	CacheRetention,
	Context,
	Model,
	StreamOptions,
	Tool,
} from "./types.js";
import { sanitizeSurrogates } from "./sanitize-unicode.js";
import { transformMessages } from "./transform-messages.js";
import {
	convertMessages,
	convertTools,
	normalizeToolCallId,
} from "./anthropic-convert.js";

/**
 * Resolve cache retention preference. Defaults to "short".
 */
export function resolveCacheRetention(cacheRetention?: CacheRetention): CacheRetention {
	if (cacheRetention) {
		return cacheRetention;
	}
	return "short";
}

export function getCacheControl(
	model: Model<"anthropic-messages">,
	cacheRetention?: CacheRetention,
): { retention: CacheRetention; cacheControl?: CacheControlEphemeral } {
	const retention = resolveCacheRetention(cacheRetention);
	if (retention === "none") {
		return { retention };
	}
	const ttl = retention === "long" && getAnthropicCompat(model).supportsLongCacheRetention ? "1h" : undefined;
	return {
		retention,
		cacheControl: { type: "ephemeral", ...(ttl && { ttl }) },
	};
}

export type AnthropicThinkingDisplay = "summarized" | "omitted";

export function getAnthropicCompat(
	model: Model<"anthropic-messages">,
): Required<AnthropicMessagesCompat> {
	return {
		supportsEagerToolInputStreaming: model.compat?.supportsEagerToolInputStreaming ?? true,
		supportsLongCacheRetention: model.compat?.supportsLongCacheRetention ?? true,
		sendSessionAffinityHeaders: model.compat?.sendSessionAffinityHeaders ?? false,
		supportsCacheControlOnTools: model.compat?.supportsCacheControlOnTools ?? true,
		supportsTemperature: model.compat?.supportsTemperature ?? true,
		allowEmptySignature: model.compat?.allowEmptySignature ?? false,
		supportsStrictTools: model.compat?.supportsStrictTools ?? false,
	};
}

export interface AnthropicOptions extends StreamOptions {
	thinkingEnabled?: boolean;
	thinkingBudgetTokens?: number;
	thinkingDisplay?: AnthropicThinkingDisplay;
	interleavedThinking?: boolean;
	toolChoice?: "auto" | "any" | "none" | { type: "tool"; name: string };
}

export function buildParams(
	model: Model<"anthropic-messages">,
	context: Context,
	options?: AnthropicOptions,
): MessageCreateParamsStreaming {
	const { cacheControl } = getCacheControl(model, options?.cacheRetention);
	const compat = getAnthropicCompat(model);
	const transformedMessages = transformMessages(context.messages, model, normalizeToolCallId);
	const immediateTools = uniqueTools(context.tools);
	const params: MessageCreateParamsStreaming = {
		model: model.id,
		messages: convertMessages(
			transformedMessages,
			cacheControl,
			compat.allowEmptySignature,
		),
		max_tokens: options?.maxTokens ?? model.maxTokens,
		stream: true,
	};

	if (context.systemPrompt) {
		params.system = [
			{
				type: "text",
				text: sanitizeSurrogates(context.systemPrompt),
				...(cacheControl ? { cache_control: cacheControl } : {}),
			},
		];
	}

	// Temperature is incompatible with extended thinking and unsupported on Claude Opus 4.7+.
	if (options?.temperature !== undefined && !options?.thinkingEnabled && compat.supportsTemperature) {
		params.temperature = options.temperature;
	}

	if (immediateTools.length > 0) {
		params.tools = convertTools(
			immediateTools,
			compat.supportsEagerToolInputStreaming,
			compat.supportsStrictTools,
			compat.supportsCacheControlOnTools ? cacheControl : undefined,
		);
	}

	if (model.reasoning) {
		if (options?.thinkingEnabled) {
			const display: AnthropicThinkingDisplay = options.thinkingDisplay ?? "summarized";
			params.thinking = {
				type: "enabled",
				budget_tokens: options.thinkingBudgetTokens || 1024,
				display,
			};
		} else if (options?.thinkingEnabled === false && model.thinkingLevelMap?.off !== null) {
			params.thinking = { type: "disabled" };
		}
	}

	if (options?.metadata) {
		const userId = options.metadata.user_id;
		if (typeof userId === "string") {
			params.metadata = { user_id: userId };
		}
	}

	if (options?.toolChoice) {
		if (typeof options.toolChoice === "string") {
			params.tool_choice = { type: options.toolChoice };
		} else {
			params.tool_choice = options.toolChoice;
		}
	}

	return params;
}

export function shouldUseFineGrainedToolStreamingBeta(model: Model<"anthropic-messages">, context: Context): boolean {
	return !!context.tools?.length && !getAnthropicCompat(model).supportsEagerToolInputStreaming;
}

function uniqueTools(tools: Tool[] | undefined): Tool[] {
	const unique = new Map<string, Tool>();
	for (const tool of tools ?? []) {
		unique.set(tool.name, tool);
	}
	return [...unique.values()];
}
