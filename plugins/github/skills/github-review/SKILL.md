---
name: github-review
description: Read GitHub pull request context and repository source with the official GitHub MCP, alongside Host-pinned local Git evidence.
---

Search the GitHub plugin with a targeted query. Inspect the relevant connection,
then request the exact tool reference if its full input schema was not returned.
Invoke it through Code Mode's `plugin(reference, arguments)` or the direct plugin
executor when Code Mode is unavailable. Use the returned schema and exact names.

Read the pull request, its comments, and repository metadata through GitHub MCP.
For source evidence, use the full immutable commit SHA given by the Host; branch
names and a pull request's current head can change during a review. Inspect pinned
local commits with the Renoa Git plugin when available. Report differences between
current GitHub metadata and the Host's pinned review snapshot.

This package uses GitHub's read-only endpoint. Credentials belong to Host account
connections, never this package. Authorize the relevant account through
`plugin_manage`. The Host review pipeline validates findings and controls review
publication; finish with the requested report instead of posting independently.

Endpoint source: https://github.com/github/github-mcp-server/blob/main/docs/server-configuration.md
