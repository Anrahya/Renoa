import { createHash } from "node:crypto";
const spec = JSON.stringify({ id: "fixture" });
if (process.env.RENOA_MODEL_ACTION === "catalog") {
  process.stdout.write(JSON.stringify({ ok: true, response: { models: [{
    id: "fixture", name: "Fixture", reasoning_levels: ["high"],
    context_window_tokens: 500000, model_spec: { id: "fixture" }
  }] } }));
  process.exit(0);
}
if (process.env.RENOA_MODEL_ACTION === "describe") {
  process.stdout.write(JSON.stringify({ ok: true, response: {
    context_window_tokens: 500000, max_output_tokens: 32768, model_spec: spec,
    model_binding_id: createHash("sha256").update(spec).digest("hex"), reasoning_level: "high"
  } }));
  process.exit(0);
}
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
const fail = message => { throw new Error(message); };
if (input.includes("fixture-shared-host-secret")) fail("credential reached the model");
const index = request.messages.findLastIndex(message => message.role === "user");
const prompt = request.messages[index].content[0].text;
const results = request.messages.slice(index + 1).filter(message => message.role === "tool");
for (const result of results) if (result.result.is_error) fail(JSON.stringify(result.result));
const value = i => JSON.parse(results[i].result.content[0].text);
const complete = (content, stop_reason = "stop") => process.stdout.write(JSON.stringify({
  event: "completed", response: { content, stop_reason,
    usage: { input: 10, output: 2, cache_read: 0, cache_write: 0 },
    metadata: { api: "test", provider: "xai", model: "fixture" }
  }
}) + "\n");
const call = (name, arguments_) => complete([{type:"tool_call",id:`shared-${results.length}`,name,arguments:arguments_}], "tool_use");
const finish = () => complete([{type:"text",text:"Shared capabilities ready."}]);
if (prompt.startsWith("Reuse ")) {
  const digest = prompt.slice(6);
  switch (results.length) {
    case 0: call("extension_manage", {action:"list"}); break;
    case 1: {
      const inventory = value(0).items;
      if (!inventory.some(item => item.kind === "package" && item.package_digest === digest)) fail("package missing from shared library");
      const connection = inventory.find(item => item.kind === "connection" && item.connection === "shared-x");
      if (!connection || connection.enabled_for_profile || !connection.catalog_loaded) fail("profile did not see reusable connection");
      call("extension_manage", {action:"enable",connection:"shared-x"}); break;
    }
    case 2: call("extension_manage", {action:"add",source:{kind:"installed",package_digest:digest}}); break;
    case 3:
      if (value(2).source !== "installed" || value(2).skills.accepted[0] !== "shared-workflow") fail("installed skill reuse failed");
      call("skill_search", {query:"shared-workflow"}); break;
    case 4:
      if (!value(3).some(item => item.name === "shared-workflow")) fail("shared skill absent");
      call("skill_load", {name:"shared-workflow"}); break;
    case 5: call("tool_search", {query:"shared_echo"}); break;
    case 6: call("tool_load", {references:[value(5).matches[0].reference]}); break;
    case 7: call("tool_execute", {reference:value(5).matches[0].reference,arguments:{}}); break;
    case 8:
      if (!results[7].result.content[0].text.includes("Authenticated shared tool succeeded")) fail("shared credential invocation failed");
      finish(); break;
    default: fail("unexpected reuse turn");
  }
} else if (prompt === "Confirm") {
  if (!request.system_prompt.includes("SHARED_HOST_SKILL_INSTRUCTIONS")) fail("pinned shared skill missing after restart");
  if (results.length === 0) call("tool_search", {query:"shared_echo"});
  else if (results.length === 1) call("tool_load", {references:[value(0).matches[0].reference]});
  else if (results.length === 2) call("tool_execute", {reference:value(0).matches[0].reference,arguments:{}});
  else if (results.length === 3) finish();
  else fail("unexpected confirmation turn");
} else fail("unexpected prompt");
