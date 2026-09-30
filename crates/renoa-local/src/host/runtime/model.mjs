import {createHash} from "node:crypto";
const id=process.env.RENOA_MODEL ?? "fallback";
const spec=JSON.stringify({id});
if(process.env.RENOA_MODEL_ACTION==="catalog") {
 process.stdout.write(JSON.stringify({ok:true,response:{models:["fallback","selected"].map(id=>({id,name:id,reasoning_levels:["high"],context_window_tokens:500000,model_spec:{id}}))}}));
 process.exit(0);
}
if(process.env.RENOA_MODEL_ACTION==="describe") {
 process.stdout.write(JSON.stringify({ok:true,response:{context_window_tokens:500000,max_output_tokens:32768,model_spec:spec,model_binding_id:createHash("sha256").update(spec).digest("hex"),reasoning_level:"high"}}));
 process.exit(0);
}
let input="";for await(const chunk of process.stdin) input+=chunk;
const request=JSON.parse(input);
const names=request.tools.map(tool=>tool.name).sort();
const code=names.includes("code_mode");
if(JSON.stringify(names)!==JSON.stringify([code?"code_mode":"tool_execute","plugin_manage","plugin_search"].sort())) throw Error(`Unexpected tools ${names}`);
const lastUser=request.messages.findLastIndex(message=>message.role==="user");
const results=request.messages.slice(lastUser+1).filter(message=>message.role==="tool").map(message=>message.result);
const call=(id,name,args)=>finish([{type:"tool_call",id,name,arguments:args}],"tool_use");
function finish(content,stop_reason="stop") {
 process.stdout.write(JSON.stringify({event:"completed",response:{content,stop_reason,usage:{input:10,output:2,cache_read:0,cache_write:0},metadata:{api:"test",provider:"xai",model:id}}})+"\n");
}
const prompt=request.messages[lastUser].content[0].text;
if(prompt==="Check current review guidance.") {
 if((JSON.stringify(request).match(/PINNED_REVIEW_INSTRUCTION/g)??[]).length!==1) throw Error("Duplicated or missing active skill body");
 finish([{type:"text",text:"Review guidance appears once."}]);
} else if(prompt==="Which profile do you see?") {
 finish([{type:"text",text:(JSON.stringify(request).match(/PROFILE_[A-Z]+/g)??["none"]).join(",")}]);
} else if(results.length===0) call("targeted-search","plugin_search",{query:prompt==="Activate a review skill."?"skill_load":"agent_manage"});
else {
 const last=results.at(-1);if(last.is_error) throw Error(JSON.stringify(last));
 const value=JSON.parse(last.content[0].text);
 if(last.name==="plugin_search") {
  const match=value.tool_matches?.find(match=>match.name===(prompt==="Activate a review skill."?"skill_load":"agent_manage")) ?? value;
  if(!match.reference) throw Error("No exact reference");
  if(!match.input_schema) call("exact-schema","plugin_search",{reference:match.reference});
  else {
   const args=prompt==="Activate a review skill."?{name:"review"}:{action:"create",name:"Child",instructions:"Perform the assigned task.",tools:[]};
   if(code) call("create-child","code_mode",{source:(prompt==="Activate a review skill."?`await plugin(${JSON.stringify(value.tool_matches.find(tool=>tool.name==="skill_search").reference)}, {"query":"review"})\n`:"")+`result = await plugin(${JSON.stringify(match.reference)}, ${JSON.stringify(args)})\nresult`});
   else call("create-child","tool_execute",{reference:match.reference,arguments:args});
  }
 } else {
  if(prompt==="Activate a review skill.") {
   if(!value.content[0].text.includes("PINNED_REVIEW_INSTRUCTION")) throw Error("Missing skill activation");
   finish([{type:"text",text:"Review skill activated."}]);
  } else {
   const child=code?JSON.parse(value.content[0].text):value;
   if(child.name!=="Child" || child.tools.length!==0) throw Error("Wrong child definition");
   finish([{type:"text",text:"Child created without machine access."}]);
  }
 }
}
