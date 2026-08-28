use std::{fs, path::Path};

pub(super) fn write_catalog_adapter(path: &Path, actions: &Path) {
    fs::write(
        path,
        format!(
            r"
import fs from 'node:fs';
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
fs.appendFileSync({}, `${{request.action}}\n`);
const candidate = {{
  reference: 'integrations.sh/exa.ai/exa-mcp-server/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
  name: 'Exa MCP Server',
  description: 'Current web search through Exa.',
  domain: 'exa.ai',
  server: 'exa-mcp-server',
  endpoint: 'https://mcp.exa.ai/mcp',
  transport: 'streamable-http',
  docs: 'https://exa.ai/docs/reference/exa-mcp',
  auth: {{status: 'none'}},
  source: {{
    provider: 'integrations.sh',
    record: 'https://integrations.sh/exa.ai/',
    evidence: ['https://exa.ai/docs/reference/exa-mcp']
  }}
}};
const result = request.action === 'search'
  ? {{action: 'search', candidates: [candidate]}}
  : {{action: 'resolve', candidate}};
process.stdout.write(JSON.stringify({{
  wire_version: 1,
  event: 'completed',
  adapter_revision: 'integration-catalog-node-v0.1.0',
  result
}}) + '\n');
",
            serde_json::to_string(&actions.to_string_lossy()).expect("encode actions path")
        ),
    )
    .expect("write catalog adapter");
}

pub(super) fn write_mcp_adapter(path: &Path) {
    fs::write(
        path,
        r"
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
process.stdout.write(JSON.stringify({
  wire_version: 4,
  event: 'discovered',
  catalog: {
    endpoint: request.endpoint,
    protocol_version: '2026-07-28',
    adapter_revision: 'mcp-client-node-v0.4.0',
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

pub(super) fn write_single_candidate_catalog_adapter(path: &Path, reference: &str) {
    fs::write(
        path,
        format!(
            r"
let input = '';
for await (const chunk of process.stdin) input += chunk;
const request = JSON.parse(input);
const candidate = {{
  reference: {reference},
  name: 'Exa MCP Server',
  description: 'Current web search through Exa.',
  domain: 'exa.ai',
  server: 'exa-mcp-server',
  endpoint: 'https://mcp.exa.ai/mcp',
  transport: 'streamable-http',
  auth: {{status: 'none'}},
  source: {{provider: 'integrations.sh', record: 'https://integrations.sh/exa.ai/', evidence: []}}
}};
process.stdout.write(JSON.stringify({{
  wire_version: 1,
  event: 'completed',
  adapter_revision: 'integration-catalog-node-v0.1.0',
  result: request.action === 'search'
    ? {{action: 'search', candidates: [candidate]}}
    : {{action: 'resolve', candidate}}
}}) + '\n');
",
            reference = serde_json::to_string(reference).expect("encode reference")
        ),
    )
    .expect("write catalog adapter");
}

pub(super) fn write_failed_mcp_adapter(path: &Path) {
    fs::write(
        path,
        r"
for await (const _chunk of process.stdin) {}
process.stdout.write(JSON.stringify({
  wire_version: 4,
  event: 'failed',
  failure: {
    kind: 'incompatible_protocol',
    certainty: 'definite',
    message: 'The endpoint supports no usable MCP version.',
    partial_changes_possible: false,
    diagnostic: {code: 'ERA_NEGOTIATION_FAILED', detail: 'server rejected every supported revision'}
  }
}) + '\n');
",
    )
    .expect("write failed MCP adapter");
}
