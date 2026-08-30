use renoa_agent::ToolError;
use serde::Serialize;

use crate::{
    mcp::McpConnectionStatus,
    mcp::hex_sha256,
    output::MAX_TOOL_OUTPUT_BYTES,
    plugins::{PluginListReport, PluginNotice},
    skills::SkillSourceReport,
};

pub(super) const DEFAULT_LIST_LIMIT: usize = 32;
pub(super) const MAX_LIST_LIMIT: usize = 32;

pub(super) const fn default_list_limit() -> usize {
    DEFAULT_LIST_LIMIT
}

#[derive(Serialize)]
pub(super) struct ExtensionListPage<'a> {
    returned: usize,
    total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
    items: Vec<ExtensionInventoryItem<'a>>,
}

impl<'a> ExtensionListPage<'a> {
    pub(super) fn new(
        packages: &'a PluginListReport,
        connections: &'a [McpConnectionStatus],
        skill_sources: &'a [SkillSourceReport],
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Self, ToolError> {
        let inventory = inventory(packages, connections, skill_sources);
        let total = inventory.len();
        let encoded = serde_json::to_vec(&inventory).map_err(|error| {
            ToolError::internal(format!(
                "extension inventory could not be fingerprinted: {error}"
            ))
        })?;
        let revision = hex_sha256(&encoded);
        let offset = parse_cursor(cursor, &revision, total)?;
        let mut items = inventory
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        loop {
            let returned = items.len();
            let consumed = offset.saturating_add(returned);
            let next_cursor = (consumed < total).then(|| format!("{revision}:{consumed}"));
            let page = Self {
                returned,
                total,
                next_cursor,
                items,
            };
            let encoded = serde_json::to_vec(&page).map_err(|error| {
                ToolError::internal(format!(
                    "extension inventory page could not be encoded: {error}"
                ))
            })?;
            if encoded.len() <= MAX_TOOL_OUTPUT_BYTES {
                return Ok(page);
            }
            if page.items.len() <= 1 {
                return Err(ToolError::output_limit(format!(
                    "one extension inventory fact exceeds the {MAX_TOOL_OUTPUT_BYTES}-byte tool output boundary"
                )));
            }
            items = page.items;
            items.pop();
        }
    }
}

fn parse_cursor(cursor: Option<&str>, revision: &str, total: usize) -> Result<usize, ToolError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let Some((cursor_revision, offset)) = cursor.split_once(':') else {
        return Err(invalid_cursor());
    };
    if cursor_revision.len() != 64
        || !cursor_revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_cursor());
    }
    let offset = offset.parse::<usize>().map_err(|_| invalid_cursor())?;
    if cursor_revision != revision {
        return Err(ToolError::conflict(
            "extension inventory changed while it was being listed; restart from the first page without a cursor",
        ));
    }
    if offset >= total {
        return Err(invalid_cursor());
    }
    Ok(offset)
}

fn invalid_cursor() -> ToolError {
    ToolError::invalid_input(
        "list cursor is invalid; pass next_cursor unchanged or omit it to restart from the first page",
    )
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ExtensionInventoryItem<'a> {
    Package {
        package_digest: &'a str,
        name: &'a str,
        version: Option<&'a str>,
        mcp_server_count: usize,
        notice_count: usize,
    },
    PackageMcpServer {
        package_digest: &'a str,
        server: &'a str,
    },
    PackageNotice {
        package_digest: &'a str,
        #[serde(flatten)]
        notice: &'a PluginNotice,
    },
    PackageRejection {
        package_digest: &'a str,
        reason: &'a str,
    },
    Connection {
        #[serde(flatten)]
        status: &'a McpConnectionStatus,
    },
    PluginSkillSource {
        source: &'a str,
        accepted_count: usize,
        rejected_count: usize,
    },
    PluginSkill {
        source: &'a str,
        name: &'a str,
    },
    PluginSkillRejection {
        source: &'a str,
        entry: &'a str,
        reason: &'a str,
    },
}

fn inventory<'a>(
    packages: &'a PluginListReport,
    connections: &'a [McpConnectionStatus],
    skill_sources: &'a [SkillSourceReport],
) -> Vec<ExtensionInventoryItem<'a>> {
    let mut items = Vec::new();
    for package in packages.installed() {
        items.push(ExtensionInventoryItem::Package {
            package_digest: package.digest(),
            name: package.metadata().name(),
            version: package.metadata().version(),
            mcp_server_count: package.mcp_servers().len(),
            notice_count: package.notices().len(),
        });
        items.extend(package.mcp_servers().iter().map(|server| {
            ExtensionInventoryItem::PackageMcpServer {
                package_digest: package.digest(),
                server: server.id(),
            }
        }));
        items.extend(package.notices().iter().map(|notice| {
            ExtensionInventoryItem::PackageNotice {
                package_digest: package.digest(),
                notice,
            }
        }));
    }
    items.extend(packages.rejected().iter().map(|rejected| {
        ExtensionInventoryItem::PackageRejection {
            package_digest: rejected.package_digest(),
            reason: rejected.reason(),
        }
    }));
    items.extend(
        connections
            .iter()
            .map(|status| ExtensionInventoryItem::Connection { status }),
    );
    for source in skill_sources {
        let components = source.components();
        items.push(ExtensionInventoryItem::PluginSkillSource {
            source: source.source(),
            accepted_count: components.accepted().len(),
            rejected_count: components.rejected().len(),
        });
        items.extend(components.accepted().iter().map(|name| {
            ExtensionInventoryItem::PluginSkill {
                source: source.source(),
                name,
            }
        }));
        items.extend(components.rejected().iter().map(|rejection| {
            ExtensionInventoryItem::PluginSkillRejection {
                source: source.source(),
                entry: rejection.entry(),
                reason: rejection.reason(),
            }
        }));
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::PluginListRejection;

    #[test]
    fn a_page_shrinks_to_the_output_boundary_without_truncating_a_fact() {
        let reason = "x".repeat(4 * 1_024);
        let rejected = (0..32)
            .map(|index| PluginListRejection {
                package_digest: format!("{index:064x}"),
                reason: reason.clone(),
            })
            .collect();
        let packages = PluginListReport::new(Vec::new(), rejected);
        let page = ExtensionListPage::new(&packages, &[], &[], None, MAX_LIST_LIMIT)
            .expect("bounded inventory page");
        let encoded = serde_json::to_vec(&page).expect("encode inventory page");
        assert!(encoded.len() <= MAX_TOOL_OUTPUT_BYTES);
        assert!(page.returned < MAX_LIST_LIMIT);
        assert!(page.next_cursor.is_some());
        let value = serde_json::to_value(page).expect("encode inventory value");
        let first_reason = value["items"][0]["reason"]
            .as_str()
            .expect("rejection retains its reason");
        assert_eq!(first_reason, reason);
    }

    #[test]
    fn an_invalid_cursor_never_becomes_an_empty_page() {
        let packages = PluginListReport::new(Vec::new(), Vec::new());
        let Err(error) = ExtensionListPage::new(&packages, &[], &[], Some("not-a-cursor"), 1)
        else {
            panic!("malformed cursor must fail")
        };
        assert_eq!(error.code(), renoa_agent::ToolErrorCode::InvalidInput);
    }
}
