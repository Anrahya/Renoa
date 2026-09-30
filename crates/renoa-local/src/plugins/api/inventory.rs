use super::super::PluginError;
use serde::Serialize;

use crate::{
    mcp::McpConnectionStatus,
    mcp::hex_sha256,
    output::MAX_TOOL_OUTPUT_BYTES,
    plugins::{PluginListReport, PluginNotice},
    skills::SkillSourceReport,
};

pub(crate) const MAX_LIST_LIMIT: usize = super::MAX_PLUGIN_PAGE;

#[derive(Serialize)]
pub struct PluginInventoryPage {
    returned: usize,
    total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
    items: Vec<PluginInventoryItem>,
}

impl PluginInventoryPage {
    #[must_use]
    pub const fn returned(&self) -> usize {
        self.returned
    }
    #[must_use]
    pub const fn total(&self) -> usize {
        self.total
    }
    #[must_use]
    pub fn next_cursor(&self) -> Option<&str> {
        self.next_cursor.as_deref()
    }
    #[must_use]
    pub fn items(&self) -> &[PluginInventoryItem] {
        &self.items
    }
    pub(crate) fn new(
        packages: &PluginListReport,
        connections: &[McpConnectionStatus],
        skill_sources: &[SkillSourceReport],
        activations: &[crate::plugins::PluginActivation],
        host_plugins: &[(
            crate::plugins::host::state::HostPluginActivation,
            Option<serde_json::Value>,
        )],
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Self, PluginError> {
        if !(1..=MAX_LIST_LIMIT).contains(&limit) {
            return Err(PluginError::Invalid(format!(
                "list limit must be between 1 and {MAX_LIST_LIMIT}"
            )));
        }
        let mut inventory = inventory(packages, connections, skill_sources);
        inventory.extend(
            activations
                .iter()
                .cloned()
                .map(|activation| PluginInventoryItem::Activation { activation }),
        );
        inventory.extend(host_plugins.iter().cloned().map(|(activation, settings)| {
            PluginInventoryItem::HostPlugin {
                activation,
                settings,
            }
        }));
        let total = inventory.len();
        let encoded = serde_json::to_vec(&inventory).map_err(|error| {
            PluginError::Unavailable(format!(
                "plugin inventory could not be fingerprinted: {error}"
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
                PluginError::Unavailable(format!(
                    "plugin inventory page could not be encoded: {error}"
                ))
            })?;
            if encoded.len() <= MAX_TOOL_OUTPUT_BYTES {
                return Ok(page);
            }
            if page.items.len() <= 1 {
                return Err(PluginError::OutputLimit(format!(
                    "one plugin inventory fact exceeds the {MAX_TOOL_OUTPUT_BYTES}-byte tool output boundary"
                )));
            }
            items = page.items;
            items.pop();
        }
    }
}

fn parse_cursor(cursor: Option<&str>, revision: &str, total: usize) -> Result<usize, PluginError> {
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
        return Err(PluginError::Conflict(
            "plugin inventory changed while it was being listed; restart from the first page without a cursor".to_owned(),
        ));
    }
    if offset >= total {
        return Err(invalid_cursor());
    }
    Ok(offset)
}

fn invalid_cursor() -> PluginError {
    PluginError::Invalid(
        "list cursor is invalid; pass next_cursor unchanged or omit it to restart from the first page".to_owned(),
    )
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginInventoryItem {
    HostPlugin {
        #[serde(flatten)]
        activation: crate::plugins::host::state::HostPluginActivation,
        /// This agent's settings, for a plugin that takes them; `{}` is the defaults.
        #[serde(skip_serializing_if = "Option::is_none")]
        settings: Option<serde_json::Value>,
    },
    Activation {
        #[serde(flatten)]
        activation: crate::plugins::PluginActivation,
    },
    Package {
        package_digest: String,
        name: String,
        version: Option<String>,
        mcp_server_count: usize,
        notice_count: usize,
    },
    PackageMcpServer {
        package_digest: String,
        server: String,
    },
    PackageNotice {
        package_digest: String,
        #[serde(flatten)]
        notice: PluginNotice,
    },
    PackageRejection {
        package_digest: String,
        reason: String,
    },
    Connection {
        #[serde(flatten)]
        status: McpConnectionStatus,
    },
    PluginSkillSource {
        source: String,
        accepted_count: usize,
        rejected_count: usize,
    },
    PluginSkill {
        source: String,
        name: String,
    },
    PluginSkillRejection {
        source: String,
        entry: String,
        reason: String,
    },
}

fn inventory(
    packages: &PluginListReport,
    connections: &[McpConnectionStatus],
    skill_sources: &[SkillSourceReport],
) -> Vec<PluginInventoryItem> {
    let mut items = Vec::new();
    for package in packages.installed() {
        items.push(PluginInventoryItem::Package {
            package_digest: package.digest().to_owned(),
            name: package.metadata().name().to_owned(),
            version: package.metadata().version().map(str::to_owned),
            mcp_server_count: package.mcp_servers().len(),
            notice_count: package.notices().len(),
        });
        items.extend(package.mcp_servers().iter().map(|server| {
            PluginInventoryItem::PackageMcpServer {
                package_digest: package.digest().to_owned(),
                server: server.id().to_owned(),
            }
        }));
        items.extend(
            package
                .notices()
                .iter()
                .map(|notice| PluginInventoryItem::PackageNotice {
                    package_digest: package.digest().to_owned(),
                    notice: notice.clone(),
                }),
        );
    }
    items.extend(packages.rejected().iter().map(|rejected| {
        PluginInventoryItem::PackageRejection {
            package_digest: rejected.package_digest().to_owned(),
            reason: rejected.reason().to_owned(),
        }
    }));
    items.extend(
        connections
            .iter()
            .map(|status| PluginInventoryItem::Connection {
                status: status.clone(),
            }),
    );
    for source in skill_sources {
        let components = source.components();
        items.push(PluginInventoryItem::PluginSkillSource {
            source: source.source().to_owned(),
            accepted_count: components.accepted().len(),
            rejected_count: components.rejected().len(),
        });
        items.extend(
            components
                .accepted()
                .iter()
                .map(|name| PluginInventoryItem::PluginSkill {
                    source: source.source().to_owned(),
                    name: name.clone(),
                }),
        );
        items.extend(components.rejected().iter().map(|rejection| {
            PluginInventoryItem::PluginSkillRejection {
                source: source.source().to_owned(),
                entry: rejection.entry().to_owned(),
                reason: rejection.reason().to_owned(),
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
        let page = PluginInventoryPage::new(&packages, &[], &[], &[], &[], None, MAX_LIST_LIMIT)
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
        let Err(error) =
            PluginInventoryPage::new(&packages, &[], &[], &[], &[], Some("not-a-cursor"), 1)
        else {
            panic!("malformed cursor must fail")
        };
        assert!(matches!(error, PluginError::Invalid(_)));
    }
}
