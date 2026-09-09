import { createHash } from "node:crypto";
import { readFileSync, appendFileSync, writeFileSync } from "node:fs";
const spec = JSON.stringify({id:readFileSync(process.env.RENOA_MODEL_AUTH_STORE,"utf8")==="drift"?"changed":"fixture"});
if (process.env.RENOA_MODEL_ACTION === "catalog") {
  console.log(JSON.stringify({ok:true,response:{models:[{id:"fixture",name:"Fixture",reasoning_levels:["high"],context_window_tokens:500000,model_spec:{id:"fixture"}}]}}));
} else if (process.env.RENOA_MODEL_ACTION === "describe") {
  console.log(JSON.stringify({ok:true,response:{context_window_tokens:500000,max_output_tokens:32768,model_spec:spec,model_binding_id:createHash("sha256").update(spec).digest("hex"),reasoning_level:"high"}}));
} else {
  let input="";for await (const part of process.stdin) input+=part;
  const request=JSON.parse(input);
  const mode=readFileSync(process.env.RENOA_MODEL_AUTH_STORE,"utf8");
  if(mode==="git") {
    appendFileSync(process.env.RENOA_MODEL_AUTH_STORE+".calls",process.env.RENOA_MODEL_SESSION_ID+"\n");
    const {run}=await import("./git_model.mjs");
    run(request,(content,stop_reason="stop")=>console.log(JSON.stringify({event:"completed",response:{content,stop_reason,usage:{input:10,output:2,cache_read:5,cache_write:0},metadata:{api:"fixture",provider:"xai",model:"fixture"}}})));
    process.exit(0);
  }
  if(mode==="compactions" && request.tools.length===0) {
    appendFileSync(process.env.RENOA_MODEL_AUTH_STORE+".compactions", "summary\n");
    console.log(JSON.stringify({event:"completed",response:{content:[{type:"text",text:["Goal and user intent","Hard constraints and preferences","Completed work","Current state and blockers","Decisions and rationale","Exact working facts","Next action and unresolved questions"].map(h=>`## ${h}\nReview ratio at pinned head. The candidate is division by zero at src/lib.rs:2, with exact evidence:     10 / count. Check callers before reporting. Tests were not run.`).join("\n\n")}],stop_reason:"stop",usage:{input:10,output:2,cache_read:0,cache_write:0},metadata:{api:"fixture",provider:"xai",model:"fixture"}}}));
    process.exit(0);
  }
  if(mode==="large-batch") writeFileSync(process.env.RENOA_MODEL_AUTH_STORE+".last-request",input);
  if(request.tools.map(t=>t.name).join(",")!=="review_source") throw Error("reviewer inherited extra tools");
  if(!process.env.RENOA_MODEL_SESSION_ID) throw Error("missing stable session");
  if(input.includes("secret-installation-token") || input.includes("private-app-jwt")) throw Error("credential entered model context");
  appendFileSync(process.env.RENOA_MODEL_AUTH_STORE+".calls",process.env.RENOA_MODEL_SESSION_ID+"\n");
  const complete=(content,stop_reason="stop")=>console.log(JSON.stringify({event:"completed",response:{content,stop_reason,usage:{input:10,output:2,cache_read:5,cache_write:0},metadata:{api:"fixture",provider:"xai",model:"fixture"}}}));
  const last=request.messages.findLastIndex(m=>m.role==="user");
  const prompt=JSON.parse(request.messages[last].content[0].text);
  const results=request.messages.slice(last+1).filter(m=>m.role==="tool");
  const validation=prompt.task.startsWith("Validate");
  let stageCalls=0;
  if(mode==="compactions") {
    const path=process.env.RENOA_MODEL_AUTH_STORE+(validation?".validation":".investigation");
    try { stageCalls=Number(readFileSync(path,"utf8")); } catch(error) { if(error.code!=="ENOENT") throw error; }
    writeFileSync(path,String(++stageCalls));
  }
  if(!request.system_prompt.includes("Batch at most 50 tool calls")) throw Error("missing source batch budget");
  if(prompt.task.includes("model responses")) throw Error("unexpected investigation budget");
  if(mode==="invalid") complete([{type:"text",text:"This is not a structured review"}]);
  else if((mode==="compactions" && stageCalls<5) || (mode!=="compactions" && (!results.length || (mode==="exhaust" && results.length < 8)))) {
    const count=mode==="compactions"?20:(mode==="batch"||mode==="large-batch")?5:mode==="oversized-batch"?51:1;
    complete(Array.from({length:count},(_,i)=>({type:"tool_call",id:`read-${stageCalls}-${results.length}-${i}`,name:"review_source",arguments:{path:"src/lib.rs",revision:"head",start_line:1,line_count:(mode==="large-batch"||mode==="compactions")?200:10}})),"tool_use");
  } else {
    if(results.some(m=>m.result.is_error)) throw Error("source lookup failed");
    const finding={priority:"P1",path:"src/lib.rs",line:2,title:"Division by zero",trigger:"Calling ratio with a zero count",consequence:"The function panics",correction:"Handle zero before division",evidence:{path:"src/lib.rs",start_line:2,quote:"    10 / count"}};
    if(mode==="forged") finding.evidence.quote="    invented evidence";
    if(mode==="anchor") finding.line=99;
    const report={findings:[finding],limitations:[]};
    if(mode==="duplicate") report.findings.push(finding);
    if(!validation && !prompt.context.base_instructions["AGENTS.md"].includes("Trusted base convention")) throw Error("base instructions not provided");
    complete([{type:"text",text:JSON.stringify(report)}]);
  }
}
