use anyhow::Result;

use crate::config::FileRule;
use crate::dlp::disk_index::{FileIndexManager, filter_most_specific_rules};
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
        let rules = self.index.rules();
        for rule in filter_most_specific_rules(&rules, tool_text) {
            activate(session_id, &rule);
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
    use crate::config::{FileIndexOptions, FileRule, MatchMode};
    use crate::dlp::disk_index::IndexedRule;
    use std::path::PathBuf;

    #[test]
    fn path_trigger_avoids_prefix_false_positive() {
        assert!(!path_trigger_match("/secret", "/secrets-backup/file.txt"));
        assert!(path_trigger_match("/secret", "read /secret/file"));
    }

    #[test]
    fn most_specific_path_wins_over_parent() {
        let parent = IndexedRule {
            rule: FileRule {
                id: "parent".into(),
                path: PathBuf::from("/data/parent"),
                enabled: true,
                recursive: true,
                trigger_window: 3,
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            },
            normalized_path: "/data/parent".into(),
        };
        let child = IndexedRule {
            rule: FileRule {
                id: "child".into(),
                path: PathBuf::from("/data/parent/child"),
                enabled: true,
                recursive: true,
                trigger_window: 3,
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            },
            normalized_path: "/data/parent/child".into(),
        };
        let rules = vec![parent, child];
        let tool = "read_file /data/parent/child/report.txt";
        let matched = filter_most_specific_rules(&rules, tool);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "child");
    }

    #[test]
    fn parent_still_triggers_when_child_not_mentioned() {
        let parent = IndexedRule {
            rule: FileRule {
                id: "parent".into(),
                path: PathBuf::from("/data/parent"),
                enabled: true,
                recursive: true,
                trigger_window: 3,
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            },
            normalized_path: "/data/parent".into(),
        };
        let child = IndexedRule {
            rule: FileRule {
                id: "child".into(),
                path: PathBuf::from("/data/parent/child"),
                enabled: true,
                recursive: true,
                trigger_window: 3,
                match_mode: MatchMode::Fragment,
                min_fragment_len: None,
                min_fragment_ratio: None,
                formats: vec!["txt".into()],
                index: FileIndexOptions::default(),
            },
            normalized_path: "/data/parent/child".into(),
        };
        let rules = vec![parent, child];
        let tool = "read_file /data/parent/top.txt";
        let matched = filter_most_specific_rules(&rules, tool);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, "parent");
    }
}
