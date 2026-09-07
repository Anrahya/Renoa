import { createHash } from "node:crypto";
import { appendFileSync } from "node:fs";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const action = process.env.RENOA_MODEL_ACTION;
const spec = process.env.RENOA_MODEL_SPEC;
if (action === "catalog") {
  process.stdout.write(JSON.stringify({ok:true,response:{models:[{
    id:"fixture",name:"Fixture",reasoning_levels:["low","high"],
    context_window_tokens:1000000,model_spec:{id:"fixture"}
  }]}}));
} else if (action === "describe") {
  process.stdout.write(JSON.stringify({ok:true,response:{
    context_window_tokens:1000000,max_output_tokens:8192,model_spec:spec,
    model_binding_id:createHash("sha256").update(spec).digest("hex"),reasoning_level:"high"
  }}));
} else if (action === "stream") {
  const request = JSON.parse(input);
  appendFileSync(new URL("./model-requests",import.meta.url),JSON.stringify(request)+"\n");
  const specialist = request.system_prompt === "News specialist.";
  if (!specialist && !request.system_prompt.startsWith("You are Arcee, Renoa's personal operator.")) process.exit(3);
  if (!specialist && !request.tools.some(tool => tool.name === "profile_update")) process.exit(4);
  if (specialist && request.tools.some(tool => ["bash","write_file","edit_file"].includes(tool.name))) process.exit(6);
  if (!request.messages.at(-1).content.some(part => part.text?.includes("<turn_context>"))) process.exit(5);
  appendFileSync(new URL("./model-calls",import.meta.url),"called\n");
  process.stdout.write(JSON.stringify({event:"completed",response:{
    content:[{type:"text",text:specialist ? "News executed this Slack request." : "Arcee executed this Slack request."}],stop_reason:"stop",
    usage:{input:8,output:4,cache_read:0,cache_write:0},
    metadata:{api:"test",provider:process.env.RENOA_MODEL_PROVIDER,model:"fixture"}
  }})+"\n");
} else process.exit(2);
