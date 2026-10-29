/**
 * Adapted from Pi packages/ai
 * https://github.com/earendil-works/pi
 * Source revision: 4a080252f06caf63a525d7873c5d94d3c23887e0 (v1.4.0)
 * License: MIT
 * Copyright (c) 2025 Mario Zechner
 */

export interface DiagnosticErrorInfo {
	name?: string;
	message: string;
	stack?: string;
	code?: string | number;
}

export interface AssistantMessageDiagnostic {
	type: string;
	timestamp: number;
	error?: DiagnosticErrorInfo;
	details?: Record<string, unknown>;
}

export function formatThrownValue(value: unknown): string {
	if (value instanceof Error) return value.message || value.name;
	if (typeof value === "string") return value;
	return String(value);
}

export function extractDiagnosticError(error: unknown): DiagnosticErrorInfo {
	if (!(error instanceof Error)) return { name: "ThrownValue", message: formatThrownValue(error) };
	const code = (error as Error & { code?: unknown }).code;
	const info: DiagnosticErrorInfo = {
		message: error.message || error.name,
	};
	if (error.name) info.name = error.name;
	if (error.stack !== undefined) info.stack = error.stack;
	if (typeof code === "string" || typeof code === "number") info.code = code;
	return info;
}

export function createAssistantMessageDiagnostic(
	type: string,
	error: unknown,
	details?: Record<string, unknown>,
): AssistantMessageDiagnostic {
	const diagnostic: AssistantMessageDiagnostic = { type, timestamp: Date.now(), error: extractDiagnosticError(error) };
	if (details !== undefined) diagnostic.details = details;
	return diagnostic;
}

export function appendAssistantMessageDiagnostic<T extends { diagnostics?: AssistantMessageDiagnostic[] }>(
	message: T,
	diagnostic: AssistantMessageDiagnostic,
): void {
	message.diagnostics = [...(message.diagnostics ?? []), diagnostic];
}
