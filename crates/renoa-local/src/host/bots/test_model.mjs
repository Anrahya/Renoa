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
  const latest=request.messages.findLast(message=>message.role==="user");
  if (latest.content[0].text === "create") {
    const result=request.messages.findLast(message=>message.role==="tool");
    if (result) {
      if (result.result.is_error) throw Error(JSON.stringify(result.result));
      complete(text(JSON.parse(result.result.content[0].text).id));
    } else complete([{type:"tool_call",id:"create-news",name:"bot_manage",arguments:{action:"create",recipe:{name:"News",instructions:"Read the news. Report only.",tools:["read_file"],connections:[]}}}],"tool_use");
  } else {
    if(request.system_prompt!=="Read the news. Report only.") throw Error("wrong specialist instructions");
    const names=request.tools.map(tool=>tool.name);
    if(!names.includes("read_file") || names.includes("bash") || names.includes("write_file") || names.includes("bot_manage") || names.includes("extension_manage")) throw Error("wrong specialist tool selection");
    if(!latest.content.some(block=>block.text?.includes("<turn_context>"))) throw Error("missing durable time context");
    complete(text("specialist ready"));
  }
}
