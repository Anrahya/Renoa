use renoa_agent::ToolError;
use serde::Serialize;

use crate::{mcp::SEARCH_RESULT_LIMIT, output::MAX_TOOL_OUTPUT_BYTES};

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
