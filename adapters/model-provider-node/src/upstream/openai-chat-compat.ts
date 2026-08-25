/**
 * Adapted from Pi packages/ai
 * https://github.com/earendil-works/pi
 * Source revision: 914cf1472e715297caa30db4b9535d534a9eb718 (v0.84.2)
 * License: MIT
 * Copyright (c) 2025 Mario Zechner
 */

import type OpenAI from "openai";
import type {
	CacheRetention,
	Model,
	OpenAICompletionsCompat,
	StreamOptions,
	ThinkingBudgets,
} from "./types.js";

export interface OpenAICompletionsOptions extends StreamOptions {
	toolChoice?: OpenAI.Chat.Completions.ChatCompletionToolChoiceOption;
	reasoningEffort?: "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
	/** Token budgets per thinking level. Only used when `compat.supportsThinkingTokenBudget` is set. */
	thinkingBudgets?: ThinkingBudgets;
}

export type ResolvedOpenAICompletionsCompat = Omit<
	Required<OpenAICompletionsCompat>,
	"cacheControlFormat" | "supportsThinkingTokenBudget"
> & {
	cacheControlFormat?: OpenAICompletionsCompat["cacheControlFormat"];
	supportsThinkingTokenBudget?: OpenAICompletionsCompat["supportsThinkingTokenBudget"];
};

export function resolveCacheRetention(cacheRetention?: CacheRetention): CacheRetention {
	if (cacheRetention) {
		return cacheRetention;
	}
	return "short";
}

/**
 * Compatibility defaults for the two advertised providers. Catalog `compat`
 * entries override these. OpenCode completions models set `maxTokensField`,
 * `thinkingFormat: "deepseek"` where required, and Responses models set
 * `sessionAffinityFormat: "openai-nosession"`.
 */
export function detectCompat(model: Model<"openai-completions">): ResolvedOpenAICompletionsCompat {
	const provider = model.provider;
	const baseUrl = model.baseUrl;
	const isXai = provider === "xai" || baseUrl.includes("api.x.ai");
	const isOpenCode =
		provider === "opencode-go" || provider === "opencode" || baseUrl.includes("opencode.ai");
	const isNonStandard = isXai || isOpenCode;

	return {
		supportsStore: !isNonStandard,
		supportsDeveloperRole: !isNonStandard,
		supportsReasoningEffort: !isXai,
		supportsUsageInStreaming: true,
		supportsFinishReason: true,
		maxTokensField: "max_completion_tokens",
		requiresToolResultName: false,
		requiresAssistantAfterToolResult: false,
		requiresThinkingAsText: false,
		requiresReasoningContentOnAssistantMessages: false,
		thinkingFormat: "openai",
		supportsThinkingTokenBudget: false,
		supportsStrictMode: true,
		sendSessionAffinityHeaders: false,
		sessionAffinityFormat: "openai",
		supportsLongCacheRetention: true,
	};
}

/**
 * Get resolved compatibility settings for a model.
 * Auto-detects from provider/URL then overrides with explicit model.compat.
 */
export function getCompat(model: Model<"openai-completions">): ResolvedOpenAICompletionsCompat {
	const detected = detectCompat(model);
	if (!model.compat) return detected;

	return {
		supportsStore: model.compat.supportsStore ?? detected.supportsStore,
		supportsDeveloperRole: model.compat.supportsDeveloperRole ?? detected.supportsDeveloperRole,
		supportsReasoningEffort: model.compat.supportsReasoningEffort ?? detected.supportsReasoningEffort,
		supportsUsageInStreaming: model.compat.supportsUsageInStreaming ?? detected.supportsUsageInStreaming,
		supportsFinishReason: model.compat.supportsFinishReason ?? detected.supportsFinishReason,
		maxTokensField: model.compat.maxTokensField ?? detected.maxTokensField,
		requiresToolResultName: model.compat.requiresToolResultName ?? detected.requiresToolResultName,
		requiresAssistantAfterToolResult:
			model.compat.requiresAssistantAfterToolResult ?? detected.requiresAssistantAfterToolResult,
		requiresThinkingAsText: model.compat.requiresThinkingAsText ?? detected.requiresThinkingAsText,
		requiresReasoningContentOnAssistantMessages:
			model.compat.requiresReasoningContentOnAssistantMessages ??
			detected.requiresReasoningContentOnAssistantMessages,
		thinkingFormat: model.compat.thinkingFormat ?? detected.thinkingFormat,
		supportsThinkingTokenBudget: model.compat.supportsThinkingTokenBudget ?? detected.supportsThinkingTokenBudget,
		supportsStrictMode: model.compat.supportsStrictMode ?? detected.supportsStrictMode,
		cacheControlFormat: model.compat.cacheControlFormat ?? detected.cacheControlFormat,
		sendSessionAffinityHeaders: model.compat.sendSessionAffinityHeaders ?? detected.sendSessionAffinityHeaders,
		sessionAffinityFormat: model.compat.sessionAffinityFormat ?? detected.sessionAffinityFormat,
		supportsLongCacheRetention: model.compat.supportsLongCacheRetention ?? detected.supportsLongCacheRetention,
	};
}
