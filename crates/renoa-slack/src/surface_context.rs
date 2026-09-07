use crate::{commands::Command, store::Work};
use renoa_agent::ContentBlock;

pub(crate) const CONTEXT: &str = include_str!("../prompts/surface-v1.md");

impl Work {
    pub(crate) fn prompt_content(&self) -> Option<Vec<ContentBlock>> {
        let Command::Prompt(text) = &self.command else {
            return None;
        };
        let mut content = vec![ContentBlock::text(text)];
        if let Some(context) = &self.surface_context {
            content.push(ContentBlock::text(context));
        }
        Some(content)
    }
}
