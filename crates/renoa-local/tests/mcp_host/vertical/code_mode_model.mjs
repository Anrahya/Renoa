const promptIndex = request.messages.findLastIndex(message => message.role === "user");
const toolResults = request.messages.slice(promptIndex + 1).filter(message => message.role === "tool");
let content;
let stopReason;
if (toolResults.length === 0) {
  content = [{ type: "tool_call", id: "code-mode-search", name: "tool_search", arguments: { query: "echo" } }];
  stopReason = "tool_use";
} else if (toolResults.length === 1 && toolResults[0].result.name === "tool_search") {
  const search = JSON.parse(toolResults[0].result.content[0].text);
  if (search.total_matches !== 1 || search.matches[0].name !== "echo" || "input_schema" in search.matches[0]) process.exit(3);
  content = [{ type: "tool_call", id: "code-mode-load", name: "tool_load", arguments: { references: [search.matches[0].reference] } }];
  stopReason = "tool_use";
} else if (toolResults.length === 2 && toolResults[1].result.name === "tool_load") {
  const loaded = JSON.parse(toolResults[1].result.content[0].text);
  if (loaded.tools.length !== 1 || loaded.tools[0].name !== "echo" || "x-mcp-header" in loaded.tools[0].input_schema.properties.tenant) process.exit(3);
  const reference = JSON.stringify(loaded.tools[0].reference);
  const source = `import asyncio\nresults = await asyncio.gather(mcp(${reference}, {'tenant': 'alpha', 'text': 'hello'}), mcp(${reference}, {'tenant': 'alpha', 'text': 'denied'}))\n[results[0]['content'][0]['text'], results[1]['is_error'], results[1]['content'][0]['text']]`;
  content = [{ type: "tool_call", id: "code-mode-outer", name: "code_mode", arguments: { source } }];
  stopReason = "tool_use";
} else if (toolResults.length === 3 && toolResults[2].result.name === "code_mode") {
  const result = toolResults[2].result;
  const value = JSON.parse(result.content[0].text);
  if (result.is_error || result.details !== null || value[0] !== "echo: hello" || value[1] !== true || !value[2].includes("HTTP 401")) process.exit(3);
  content = [{ type: "text", text: "Code Mode MCP results handled." }];
  stopReason = "stop";
} else {
  process.exit(3);
}
process.stdout.write(JSON.stringify({
  event: "completed",
  response: {
    content,
    stop_reason: stopReason,
    usage: { input: 10, output: 2, cache_read: 0, cache_write: 0 },
    metadata: { api: "test", provider: "xai", model: "fixture-model" }
  }
}) + "\n");
