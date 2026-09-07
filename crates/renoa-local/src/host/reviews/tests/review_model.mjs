import { createHash } from "node:crypto";
import { readFileSync, appendFileSync } from "node:fs";
const spec = JSON.stringify({id:readFileSync(process.env.RENOA_MODEL_AUTH_STORE,"utf8")==="drift"?"changed":"fixture"});
if (process.env.RENOA_MODEL_ACTION === "catalog") {
  console.log(JSON.stringify({ok:true,response:{models:[{id:"fixture",name:"Fixture",reasoning_levels:["high"],context_window_tokens:500000,model_spec:{id:"fixture"}}]}}));
} else if (process.env.RENOA_MODEL_ACTION === "describe") {
  console.log(JSON.stringify({ok:true,response:{context_window_tokens:500000,max_output_tokens:32768,model_spec:spec,model_binding_id:createHash("sha256").update(spec).digest("hex"),reasoning_level:"high"}}));
} else {
  let input="";for await (const part of process.stdin) input+=part;
  const request=JSON.parse(input);
  const mode=readFileSync(process.env.RENOA_MODEL_AUTH_STORE,"utf8");
  if(request.tools.map(t=>t.name).join(",")!=="review_source") throw Error("reviewer inherited extra tools");
  if(!process.env.RENOA_MODEL_SESSION_ID) throw Error("missing stable session");
  if(input.includes("secret-installation-token") || input.includes("private-app-jwt")) throw Error("credential entered model context");
  appendFileSync(process.env.RENOA_MODEL_AUTH_STORE+".calls",process.env.RENOA_MODEL_SESSION_ID+"\n");
  const complete=(content,stop_reason="stop")=>console.log(JSON.stringify({event:"completed",response:{content,stop_reason,usage:{input:10,output:2,cache_read:5,cache_write:0},metadata:{api:"fixture",provider:"xai",model:"fixture"}}}));
  const last=request.messages.findLastIndex(m=>m.role==="user");
  const prompt=JSON.parse(request.messages[last].content[0].text);
  const results=request.messages.slice(last+1).filter(m=>m.role==="tool");
  const validation=prompt.task.startsWith("Validate");
  if(mode==="invalid") complete([{type:"text",text:"This is not a structured review"}]);
  else if(!results.length || mode==="exhaust") {
    complete([{type:"tool_call",id:"read-"+results.length,name:"review_source",arguments:{path:"src/lib.rs",revision:"head",start_line:1,line_count:10}}],"tool_use");
  } else {
    if(results.some(m=>m.result.is_error)) throw Error("source lookup failed");
    const finding={path:"src/lib.rs",line:2,title:"Division by zero",trigger:"Calling ratio with a zero count",consequence:"The function panics",correction:"Handle zero before division",evidence:{path:"src/lib.rs",start_line:2,quote:"    10 / count"}};
    if(mode==="forged") finding.evidence.quote="    invented evidence";
    if(mode==="anchor") finding.line=99;
    const report={findings:[finding],limitations:[]};
    if(mode==="duplicate") report.findings.push(finding);
    if(!validation && !prompt.context.base_instructions["AGENTS.md"].includes("Trusted base convention")) throw Error("base instructions not provided");
    complete([{type:"text",text:JSON.stringify(report)}]);
  }
}
