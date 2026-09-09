// Deterministic model boundary: request actual Git tools before producing known
// defects. No GitHub source API or partial patch is available to this model.
export function run(request, complete) {
  const names = request.tools.map(t => t.name);
  if (!names.includes("git_diff") || names.includes("bash") || names.includes("write_file")) throw Error("wrong recipe tools");
  const start = request.messages.findLastIndex(m => m.role === "user");
  const prompt = JSON.parse(request.messages[start].content[0].text);
  const first = JSON.parse(request.messages.find(m => m.role === "user").content[0].text);
  const base = first.context.merge_base_sha, head = first.head_sha;
  const results = request.messages.slice(start + 1).filter(m => m.role === "tool");
  if (results.some(m => m.result.is_error)) throw Error("real Git tool failed");
  const inventory = results.filter(m => m.result.name === "git_changes");
  const lastInventory = inventory.length ? JSON.parse(inventory.at(-1).result.content[0].text) : null;
  if (!lastInventory || lastInventory.next_offset !== null) {
    complete([{type:"tool_call",id:`inventory-${results.length}`,name:"git_changes",arguments:{base,head,offset:lastInventory?.next_offset ?? 0}}],"tool_use");
    return;
  }
  if (!results.some(m => m.result.name === "git_show")) {
    const calls = [
      ["git_diff",{base,head,path:"z-bug.rs"}], ["git_show",{commit:head,path:"z-bug.rs"}],
      ["git_diff",{base,head,path:"removed.rs"}], ["git_show",{commit:base,path:"removed.rs"}],
      ["git_show",{commit:head,path:"caller.rs"}], ["git_show",{commit:base,path:"AGENTS.md"}],
    ];
    complete(calls.map(([name,arguments_],i)=>({type:"tool_call",id:`evidence-${results.length}-${i}`,name,arguments:arguments_})),"tool_use");
    return;
  }
  const finding=(path,line,side,title,quote,evidencePath=path,evidenceLine=line,evidenceSide=side)=>({priority:"P1",path,line,side,title,
    trigger:"The change is exercised with a zero count",consequence:"The operation fails without the former guard",correction:"Restore the guard",evidence:{path:evidencePath,start_line:evidenceLine,side:evidenceSide,quote}});
  const findings = prompt.candidates?.findings ?? [
    finding("z-bug.rs",2,"head","Division by zero","    10 / count"),
    finding("removed.rs",1,"base","Removed authorization guard","check_owner();"),
    finding("caller.rs",33,"head","Changed argument reaches this caller","    10 / count","z-bug.rs",2,"head"),
  ];
  complete([{type:"text",text:JSON.stringify({findings,limitations:[]})}]);
}
