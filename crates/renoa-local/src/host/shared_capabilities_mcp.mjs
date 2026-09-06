import { appendFileSync } from "node:fs";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
if (request.wire_version !== 9 || request.credential?.secret !== "fixture-shared-host-secret") throw new Error("credential was not reused");
if (process.argv.slice(2).length || Object.values(process.env).some(value => value.includes("fixture-shared-host-secret"))) throw new Error("credential leaked outside stdin");
appendFileSync(new URL("mcp-calls", import.meta.url), request.action + "\n");
const emit = message => process.stdout.write(JSON.stringify({wire_version:9,...message}) + "\n");
if (request.action === "discover") {
  emit({event:"discovered",catalog:{
    endpoint:request.endpoint, protocol_version:"2026-07-28", adapter_revision:"mcp-client-node-v0.10.0",
    tools:[{name:"shared_echo",description:"Test shared authenticated capability",input_schema:{type:"object"},model_input_schema:{type:"object"}}],rejected_tools:[]
  }});
} else if (request.action === "call") {
  if (request.tool.name !== "shared_echo") throw new Error("unexpected tool");
  emit({event:"dispatch_started"});
  emit({event:"completed",result:{content:[{type:"text",text:"Authenticated shared tool succeeded"}],structured_content:{present:false},is_error:false}});
} else throw new Error("unexpected adapter operation");
