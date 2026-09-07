import { createHash } from "node:crypto";
const spec = JSON.stringify({id:"fixture"});
if (process.env.RENOA_MODEL_ACTION === "catalog") {
  console.log(JSON.stringify({ok:true,response:{models:[{id:"fixture",name:"Fixture",reasoning_levels:["high"],context_window_tokens:500000,model_spec:{id:"fixture"}}]}}));
} else if (process.env.RENOA_MODEL_ACTION === "describe") {
  console.log(JSON.stringify({ok:true,response:{context_window_tokens:500000,max_output_tokens:32768,model_spec:spec,model_binding_id:createHash("sha256").update(spec).digest("hex"),reasoning_level:"high"}}));
} else {
  let input="";for await (const part of process.stdin) input+=part;
  const request=JSON.parse(input);
  const complete=(content,stop_reason="stop")=>console.log(JSON.stringify({event:"completed",response:{content,stop_reason,usage:{input:10,output:1,cache_read:0,cache_write:0},metadata:{api:"fixture",provider:"xai",model:"fixture"}}}));
  const text=text=>[{type:"text",text}];
  const index=request.messages.findLastIndex(message=>message.role==="user");
  const prompt=request.messages[index].content[0].text;
  const results=request.messages.slice(index+1).filter(message=>message.role==="tool");
  const invoke=(id,name,args)=>complete([{type:"tool_call",id,name,arguments:args}],"tool_use");
  for(const result of results) if(result.result.is_error) throw Error(JSON.stringify(result.result));
  if(prompt.startsWith("create routine ") || prompt.startsWith("create once ")) {
    if(results.length) complete(text("Routine created"));
    else invoke("create-routine","routine_manage",{action:"create",spec:{agent_id:prompt.split(" ")[2],name:"Digest",prompt:"scheduled digest",schedule:prompt.startsWith("create once ")?{kind:"once",at:prompt.split(" ")[3]}:{kind:"interval",hours:12},enabled:true}});
  } else if(prompt.startsWith("reschedule ")) {
    if(!results.length) invoke("list-routines","routine_manage",{action:"list"});
    else if(results.length===1) {
      const listed=JSON.parse(results[0].result.content[0].text);
      const current=listed.routines.find(r=>r.id===prompt.split(" ")[1]);
      invoke("get-routine","routine_manage",{action:"get",id:current.id});
    } else if(results.length===2) {
      const current=JSON.parse(results[1].result.content[0].text).routine;
      invoke("update-routine","routine_manage",{action:"update",id:current.id,expected_revision:current.revision,spec:{...current.spec,schedule:{kind:"interval",hours:24}}});
    } else complete(text("Schedule updated"));
  } else if(prompt==="read latest routine result") {
    if(!results.length) invoke("list-results","routine_results",{action:"list"});
    else if(results.length===1) {
      const listed=JSON.parse(results[0].result.content[0].text);
      invoke("read-result","routine_results",{action:"read",id:listed.runs[0].id});
    } else complete(text(JSON.parse(results[1].result.content[0].text).run.output));
  } else if(prompt==="scheduled digest") {
    if(results.length) complete(text("Digest saved: digest.md"));
    else invoke("write-digest","write_file",{path:"digest.md",content:"# Digest\nSaved by the specialist."});
  } else throw Error("unexpected prompt");
}
