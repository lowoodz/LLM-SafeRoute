use anyhow::Result;

use crate::config::FileRule;
use crate::dlp::disk_index::FileIndexManager;
use crate::dlp::session::ActiveFileContent;

pub struct FileDlp {
    index: FileIndexManager,
}

impl FileDlp {
    pub fn new(rules: &[FileRule]) -> Result<Self> {
        Ok(Self {
            index: FileIndexManager::new(rules),
        })
    }

    pub fn reload(&self, rules: &[FileRule]) -> Result<()> {
        self.index.rebuild_sync(rules)
    }

    pub fn is_index_ready(&self) -> bool {
        self.index.is_ready()
    }

    pub fn check_path_triggers_in_tool_text(
        &self,
        session_id: &str,
        tool_text: &str,
        activate: impl Fn(&str, &FileRule),
    ) {
        for indexed in self.index.rules() {
            if path_trigger_match(&indexed.normalized_path, tool_text) {
                activate(session_id, &indexed.rule);
            }
        }
    }

    pub fn scan_text(&self, text: &str, active: &[ActiveFileContent]) -> String {
        self.index.scan_and_sanitize(text, active)
    }
}

/// Path must appear as a path segment, not as a prefix of a longer path token.
pub fn path_trigger_match(normalized_path: &str, tool_text: &str) -> bool {
    if normalized_path.is_empty() {
        return false;
    }
    tool_text.match_indices(normalized_path).any(|(pos, _)| {
        let before_ok = pos == 0 || !is_path_token_char(tool_text.as_bytes()[pos - 1]);
        let after_pos = pos + normalized_path.len();
        let after_ok = after_pos >= tool_text.len()
            || !is_path_token_char(tool_text.as_bytes()[after_pos]);
        before_ok && after_ok
    })
}

fn is_path_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_trigger_avoids_prefix_false_positive() {
        assert!(!path_trigger_match("/secret", "/secrets-backup/file.txt"));
        assert!(path_trigger_match("/secret", "read /secret/file"));
    }
}
