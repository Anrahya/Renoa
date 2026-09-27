// Deterministic model fixtures discover Host plugin schemas before invoking them.
const fixturePluginTools = new Set(['agent_manage', 'routine_manage', 'routine_results', 'skill_search', 'skill_load', 'agent_documents']);
let fixturePluginRequest;
const fixturePluginSchemas = new Map();
const fixtureOriginalWrite = process.stdout.write.bind(process.stdout);
function preparePluginFixture(request) {
  fixturePluginRequest = request;
  for (const message of request.messages) {
    if (message.role !== 'tool' || !message.result.call_id.startsWith('__schema_')) continue;
    if (message.result.is_error) throw Error(JSON.stringify(message.result));
    const value = JSON.parse(message.result.content[0].text);
    for (const tool of value.tool_matches ?? (value.reference ? [value] : [])) {
      if (tool.reference.startsWith('host:')) fixturePluginSchemas.set(tool.name, tool);
    }
  }
  request.messages = request.messages.filter(message => !(message.role === 'tool' && message.result.call_id.startsWith('__schema_')) && !(message.role === 'assistant' && message.content.some(part => part.id?.startsWith('__schema_'))));
  const routed = new Map();
  for (const message of request.messages) {
    if (message.role === 'assistant') for (const call of message.content) {
      if (call.type === 'tool_call' && call.name === 'tool_execute' && call.arguments.reference.startsWith('host:')) {
        const name = call.arguments.reference.split(':').at(-1);
        routed.set(call.id, name);
        call.name = name; call.arguments = call.arguments.arguments;
      }
    }
    if (message.role === 'tool' && routed.has(message.result.call_id)) message.result.name = routed.get(message.result.call_id);
  }
  return request;
}
process.stdout.write = (chunk, ...rest) => {
  let event;
  try {event = JSON.parse(chunk.toString());} catch {return fixtureOriginalWrite(chunk, ...rest);}
  if (fixturePluginRequest && event.event === 'completed') {
    event.response.content = event.response.content.map(call => {
      if (call.type !== 'tool_call' || !fixturePluginTools.has(call.name)) return call;
      const schema = fixturePluginSchemas.get(call.name);
      if (!schema?.input_schema) return {type:'tool_call',id:`__schema_${call.name}_${call.id}`,name:'plugin_search',arguments:schema ? {reference:schema.reference} : {query:call.name}};
      return {...call,name:'tool_execute',arguments:{reference:schema.reference,arguments:call.arguments}};
    });
    return fixtureOriginalWrite(JSON.stringify(event) + '\n', ...rest);
  }
  return fixtureOriginalWrite(chunk, ...rest);
};
