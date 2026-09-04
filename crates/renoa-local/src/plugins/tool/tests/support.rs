use std::{fs, path::Path};

pub(super) fn write_mcp_adapter(path: &Path) {
    fs::write(
        path,
        r"
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
process.stdout.write(JSON.stringify({
  wire_version: 9,
  event: 'discovered',
  catalog: {
    endpoint: request.endpoint,
    protocol_version: '2026-07-28',
    adapter_revision: 'mcp-client-node-v0.9.0',
    tools: [{
      name: 'web_search_exa',
      description: 'Search the web.',
      input_schema: {type: 'object', properties: {query: {type: 'string'}}},
      model_input_schema: {type: 'object', properties: {query: {type: 'string'}}}
    }],
    rejected_tools: []
  }
}) + '\n');
",
    )
    .expect("write MCP adapter");
}
