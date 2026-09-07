/// A bounded preview of the current assistant message. Status changes must not
/// replace text that the operator is reading. Final delivery owns the full text.
pub(super) struct Preview {
    text: String,
    characters: usize,
    truncated: bool,
    new_message: bool,
    pub(super) status: String,
}

const LIMIT: usize = 3500;

impl Default for Preview {
    fn default() -> Self {
        Self {
            text: String::new(),
            characters: 0,
            truncated: false,
            new_message: true,
            status: "Working…".to_owned(),
        }
    }
}

impl Preview {
    pub(super) fn start_message(&mut self) {
        // Keep the previous text until the next message actually contains text.
        // Providers can emit a message start before a long reasoning/tool phase.
        self.new_message = true;
    }

    pub(super) fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.new_message {
            self.text.clear();
            self.characters = 0;
            self.truncated = false;
            self.new_message = false;
        }
        self.status.clear();
        for character in text.chars() {
            if self.characters == LIMIT {
                self.truncated = true;
                break;
            }
            self.text.push(character);
            self.characters += 1;
        }
    }

    pub(super) fn render(&self) -> String {
        let mut text = self.text.clone();
        if self.truncated {
            text.push_str("\n\n[Reply continues; the full response will appear when finished.]");
        }
        if !self.status.is_empty() {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&self.status);
        }
        text
    }
}
