use std::collections::{HashMap, HashSet};

use renoa_agent::ToolError;
use serde::Serialize;

use crate::{
    mcp::{McpConnectionStatus, McpToolSummary, SEARCH_RESULT_LIMIT},
    output::MAX_TOOL_OUTPUT_BYTES,
    plugins::{PluginListReport, PluginMcpServer, manager::integration_id},
    skills::SkillSourceReport,
};

pub(super) struct Inventory {
    records: Vec<CardRecord>,
    tools: Vec<McpToolSummary>,
    enabled_connections: HashSet<String>,
    shared_refresh_unavailable: bool,
}

struct CardRecord {
    card: PluginCard,
    facts: Vec<PluginFact>,
    search_text: String,
}

#[derive(Clone, Serialize)]
pub(super) struct PluginCard {
    id: String,
    source: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    mcp_servers: usize,
    connections: usize,
    enabled_connections: usize,
    catalog_loaded_connections: usize,
    catalog_tool_count: usize,
    credential_configured_connections: usize,
    active_skills: usize,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum PluginFact {
    McpServer {
        server: String,
    },
    Connection {
        #[serde(flatten)]
        status: McpConnectionStatus,
        credential_configured: bool,
    },
    ActiveSkill {
        name: String,
    },
    Notice {
        component: String,
        entry: Option<String>,
        reason: String,
    },
}

#[derive(Serialize)]
pub(super) struct Page<T> {
    items: Vec<T>,
    total: usize,
    next_offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    library_refresh: Option<&'static str>,
}

impl<T: Clone + Serialize> Page<T> {
    pub(super) fn new(
        matches: Vec<T>,
        total: usize,
        offset: usize,
        shared_refresh_unavailable: bool,
    ) -> Result<Self, ToolError> {
        if offset > total {
            return Err(ToolError::invalid_input(
                "offset is beyond the available results; restart at 0",
            ));
        }
        let mut items = matches
            .into_iter()
            .take(SEARCH_RESULT_LIMIT)
            .collect::<Vec<_>>();
        loop {
            let next = offset.saturating_add(items.len());
            let page = Self {
                items,
                total,
                next_offset: (next < total).then_some(next),
                library_refresh: shared_refresh_unavailable
                    .then_some("shared library unavailable; showing local snapshot"),
            };
            let bytes = serde_json::to_vec(&page).map_err(|error| {
                ToolError::internal(format!("plugin search page could not be encoded: {error}"))
            })?;
            if bytes.len() <= MAX_TOOL_OUTPUT_BYTES {
                return Ok(page);
            }
            if page.items.len() <= 1 {
                return Err(ToolError::output_limit(format!(
                    "one plugin search result exceeds the {MAX_TOOL_OUTPUT_BYTES}-byte output boundary"
                )));
            }
            items = page.items;
            items.pop();
        }
    }

    pub(super) fn shorten(&mut self, offset: usize) -> bool {
        if self.items.len() <= 1 {
            return false;
        }
        self.items.pop();
        self.next_offset = Some(offset + self.items.len());
        true
    }
}

impl Inventory {
    pub(super) fn new(
        packages: &PluginListReport,
        connections: &[McpConnectionStatus],
        skills: &[SkillSourceReport],
        tools: Vec<McpToolSummary>,
        shared_refresh_unavailable: bool,
    ) -> Self {
        let (mut records, package_integrations) =
            Self::package_records(packages, connections, skills, &tools);
        records.extend(Self::direct_records(
            connections,
            &tools,
            &package_integrations,
        ));
        let enabled_connections = connections
            .iter()
            .filter(|status| status.enabled_for_agent())
            .map(|status| status.connection().to_owned())
            .collect();
        Self {
            records,
            tools,
            enabled_connections,
            shared_refresh_unavailable,
        }
    }

    fn package_records(
        packages: &PluginListReport,
        connections: &[McpConnectionStatus],
        skills: &[SkillSourceReport],
        tools: &[McpToolSummary],
    ) -> (Vec<CardRecord>, HashSet<String>) {
        let mut records = Vec::new();
        let mut package_integrations = HashSet::new();
        let mut name_counts = HashMap::<&str, usize>::new();
        for package in packages.installed() {
            *name_counts.entry(package.metadata().name()).or_default() += 1;
        }
        for package in packages.installed() {
            let mut facts = Vec::new();
            let integrations = package
                .mcp_servers()
                .iter()
                .map(|server| {
                    facts.push(PluginFact::McpServer {
                        server: server.id().to_owned(),
                    });
                    integration_id(package.digest(), server.id())
                })
                .collect::<HashSet<_>>();
            package_integrations.extend(integrations.iter().cloned());
            let linked = connections
                .iter()
                .filter(|status| integrations.contains(status.integration()))
                .collect::<Vec<_>>();
            facts.extend(linked.iter().map(|status| PluginFact::Connection {
                status: (*status).clone(),
                credential_configured: status.credential_configured(),
            }));
            let active_skills = if name_counts[package.metadata().name()] == 1 {
                let source = format!("agent-plugin:{}", package.metadata().name());
                skills
                    .iter()
                    .find(|report| report.source() == source)
                    .map(|report| report.components().accepted().to_vec())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            facts.extend(
                active_skills
                    .iter()
                    .map(|name| PluginFact::ActiveSkill { name: name.clone() }),
            );
            facts.extend(package.notices().iter().map(|notice| PluginFact::Notice {
                component: notice.component().to_owned(),
                entry: notice.entry().map(str::to_owned),
                reason: notice.reason().to_owned(),
            }));
            let card = PluginCard {
                id: package.digest().to_owned(),
                source: "installed_plugin",
                name: package.metadata().name().to_owned(),
                description: package.metadata().description().map(compact),
                version: package.metadata().version().map(str::to_owned),
                mcp_servers: package.mcp_servers().len(),
                connections: linked.len(),
                enabled_connections: linked
                    .iter()
                    .filter(|status| status.enabled_for_agent())
                    .count(),
                catalog_loaded_connections: linked
                    .iter()
                    .filter(|status| status.catalog_loaded())
                    .count(),
                catalog_tool_count: linked.iter().map(|status| status.tool_count()).sum(),
                credential_configured_connections: linked
                    .iter()
                    .filter(|status| status.credential_configured())
                    .count(),
                active_skills: active_skills.len(),
            };
            let mut search_text = format!(
                "{} {} {} {}",
                card.name,
                package.metadata().description().unwrap_or_default(),
                package
                    .mcp_servers()
                    .iter()
                    .map(PluginMcpServer::id)
                    .collect::<Vec<_>>()
                    .join(" "),
                active_skills.join(" ")
            );
            for tool in tools
                .iter()
                .filter(|tool| integrations.contains(tool.integration_id()))
            {
                search_text.push(' ');
                search_text.push_str(tool.name());
                search_text.push(' ');
                search_text.push_str(tool.description());
            }
            records.push(CardRecord {
                card,
                facts,
                search_text: search_text.to_lowercase(),
            });
        }
        (records, package_integrations)
    }

    fn direct_records(
        connections: &[McpConnectionStatus],
        tools: &[McpToolSummary],
        package_integrations: &HashSet<String>,
    ) -> Vec<CardRecord> {
        let mut records = Vec::new();
        let mut direct = HashMap::<&str, Vec<&McpConnectionStatus>>::new();
        for status in connections {
            if !package_integrations.contains(status.integration()) {
                direct.entry(status.integration()).or_default().push(status);
            }
        }
        for (integration, linked) in direct {
            let facts = linked
                .iter()
                .map(|status| PluginFact::Connection {
                    status: (*status).clone(),
                    credential_configured: status.credential_configured(),
                })
                .collect();
            let card = PluginCard {
                id: format!("direct:{integration}"),
                source: "direct_mcp_connection",
                name: integration.to_owned(),
                description: None,
                version: None,
                mcp_servers: 1,
                connections: linked.len(),
                enabled_connections: linked
                    .iter()
                    .filter(|status| status.enabled_for_agent())
                    .count(),
                catalog_loaded_connections: linked
                    .iter()
                    .filter(|status| status.catalog_loaded())
                    .count(),
                catalog_tool_count: linked.iter().map(|status| status.tool_count()).sum(),
                credential_configured_connections: linked
                    .iter()
                    .filter(|status| status.credential_configured())
                    .count(),
                active_skills: 0,
            };
            let mut search_text = integration.to_owned();
            for status in &linked {
                search_text.push(' ');
                search_text.push_str(status.connection());
            }
            for tool in tools
                .iter()
                .filter(|tool| tool.integration_id() == integration)
            {
                search_text.push(' ');
                search_text.push_str(tool.name());
                search_text.push(' ');
                search_text.push_str(tool.description());
            }
            records.push(CardRecord {
                card,
                facts,
                search_text: search_text.to_lowercase(),
            });
        }
        records
    }

    pub(super) fn shared_refresh_unavailable(&self) -> bool {
        self.shared_refresh_unavailable
    }

    pub(super) fn all_tools(&self) -> Vec<McpToolSummary> {
        self.tools.clone()
    }

    pub(super) fn search(&self, query: &str, offset: usize) -> Result<Page<PluginCard>, ToolError> {
        let query = validate_query(query)?;
        let browse = query == "*";
        let tokens = query
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .collect::<Vec<_>>();
        if !browse && tokens.is_empty() {
            return Err(ToolError::invalid_input(
                "plugin query must contain a letter or digit, or be `*`",
            ));
        }
        let mut matches = self
            .records
            .iter()
            .filter_map(|record| {
                if !browse
                    && !tokens
                        .iter()
                        .all(|token| record.search_text.contains(token))
                {
                    return None;
                }
                let name = record.card.name.to_lowercase();
                let score = if name == query {
                    0
                } else if name.starts_with(&query) {
                    1
                } else if name.contains(&query) {
                    2
                } else {
                    3
                };
                Some((
                    record.card.enabled_connections == 0 && record.card.active_skills == 0,
                    score,
                    &record.card,
                ))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            (left.0, left.1, &left.2.name, &left.2.id).cmp(&(
                right.0,
                right.1,
                &right.2.name,
                &right.2.id,
            ))
        });
        let total = matches.len();
        Page::new(
            matches
                .into_iter()
                .skip(offset)
                .take(SEARCH_RESULT_LIMIT)
                .map(|(_, _, card)| card.clone())
                .collect(),
            total,
            offset,
            self.shared_refresh_unavailable,
        )
    }

    pub(super) fn inspect(
        &self,
        plugin: &str,
        offset: usize,
    ) -> Result<Page<PluginFact>, ToolError> {
        let record = self
            .records
            .iter()
            .find(|record| record.card.id == plugin)
            .ok_or_else(|| {
                ToolError::invalid_input(
                    "plugin id was not found in the current Host library; search again",
                )
            })?;
        Page::new(
            record
                .facts
                .iter()
                .skip(offset)
                .take(SEARCH_RESULT_LIMIT)
                .cloned()
                .collect(),
            record.facts.len(),
            offset,
            self.shared_refresh_unavailable,
        )
    }

    pub(super) fn tools(&self, connection: &str) -> Result<Vec<McpToolSummary>, ToolError> {
        if !self.enabled_connections.contains(connection) {
            return Err(ToolError::invalid_input(
                "connection is not enabled for this agent; ask the Host to enable it or use plugin_manage when granted",
            ));
        }
        Ok(self
            .tools
            .iter()
            .filter(|tool| tool.connection_id() == connection)
            .cloned()
            .collect())
    }
}

fn validate_query(query: &str) -> Result<String, ToolError> {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.len() > 256 {
        return Err(ToolError::invalid_input(
            "plugin query must be 1-256 UTF-8 bytes",
        ));
    }
    Ok(trimmed.to_lowercase())
}

fn compact(value: &str) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(240).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests;
