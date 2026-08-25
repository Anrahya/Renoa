/**
 * Adapted from Pi packages/ai
 * https://github.com/earendil-works/pi
 * Source revision: 914cf1472e715297caa30db4b9535d534a9eb718 (v0.84.2)
 * License: MIT
 * Copyright (c) 2025 Mario Zechner
 */

import type {
	Api,
	Context,
	Model,
	SimpleStreamOptions,
	StreamOptions,
	ThinkingBudgets,
	ThinkingLevel,
} from "./types.js";
import { estimateContextTokens } from "./estimate.js";

const CONTEXT_SAFETY_TOKENS = 4096;
const MIN_MAX_TOKENS = 1;

export function clampMaxTokensToContext(model: Model<Api>, context: Context, maxTokens: number): number {
	if (model.contextWindow <= 0) return Math.max(MIN_MAX_TOKENS, maxTokens);
	const available = model.contextWindow - estimateContextTokens(context).tokens - CONTEXT_SAFETY_TOKENS;
	return Math.min(maxTokens, Math.max(MIN_MAX_TOKENS, available));
}

export function buildBaseOptions(
	model: Model<Api>,
	context: Context,
	options?: SimpleStreamOptions,
	apiKey?: string,
): StreamOptions {
	const samplingParams =
		model.samplingParams || options?.samplingParams
			? { ...model.samplingParams, ...options?.samplingParams }
			: undefined;
	const result: StreamOptions = {
		maxTokens: clampMaxTokensToContext(model, context, options?.maxTokens ?? model.maxTokens),
	};
	if (samplingParams !== undefined) result.samplingParams = samplingParams;
	if (options?.temperature !== undefined) result.temperature = options.temperature;
	if (options?.signal !== undefined) result.signal = options.signal;
	const resolvedApiKey = apiKey || options?.apiKey;
	if (resolvedApiKey !== undefined) result.apiKey = resolvedApiKey;
	if (options?.fetch !== undefined) result.fetch = options.fetch;
	if (options?.transport !== undefined) result.transport = options.transport;
	if (options?.cacheRetention !== undefined) result.cacheRetention = options.cacheRetention;
	if (options?.sessionId !== undefined) result.sessionId = options.sessionId;
	if (options?.headers !== undefined) result.headers = options.headers;
	if (options?.onPayload !== undefined) result.onPayload = options.onPayload;
	if (options?.onResponse !== undefined) result.onResponse = options.onResponse;
	if (options?.timeoutMs !== undefined) result.timeoutMs = options.timeoutMs;
	if (options?.websocketConnectTimeoutMs !== undefined) result.websocketConnectTimeoutMs = options.websocketConnectTimeoutMs;
	if (options?.maxRetries !== undefined) result.maxRetries = options.maxRetries;
	if (options?.maxRetryDelayMs !== undefined) result.maxRetryDelayMs = options.maxRetryDelayMs;
	if (options?.metadata !== undefined) result.metadata = options.metadata;
	if (options?.env !== undefined) result.env = options.env;
	return result;
}

/** Tokens always left for the answer when a thinking budget shares the response ceiling. */
export const MIN_ANSWER_TOKENS = 1024;

export function clampReasoning(effort: ThinkingLevel | undefined): Exclude<ThinkingLevel, "xhigh" | "max"> | undefined {
	return effort === "xhigh" || effort === "max" ? "high" : effort;
}

export function adjustMaxTokensForThinking(
	// Undefined means no explicit caller cap. Use the model cap and fit thinking inside it.
	baseMaxTokens: number | undefined,
	modelMaxTokens: number,
	reasoningLevel: ThinkingLevel,
	customBudgets?: ThinkingBudgets,
): { maxTokens: number; thinkingBudget: number } {
	const defaultBudgets: ThinkingBudgets = {
		minimal: 1024,
		low: 2048,
		medium: 8192,
		high: 16384,
	};
	const budgets = { ...defaultBudgets, ...customBudgets };

	const level = clampReasoning(reasoningLevel)!;
	let thinkingBudget = budgets[level]!;
	const maxTokens =
		baseMaxTokens === undefined ? modelMaxTokens : Math.min(baseMaxTokens + thinkingBudget, modelMaxTokens);

	if (maxTokens <= thinkingBudget) {
		thinkingBudget = Math.max(0, maxTokens - MIN_ANSWER_TOKENS);
	}

	return { maxTokens, thinkingBudget };
}
