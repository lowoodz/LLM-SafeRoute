mod content;
mod doc_extract;
mod file;
mod file_index;
mod fragment;
mod rg;
mod sanitize;
mod session;

pub use content::ContentDlp;
pub use file::FileDlp;
pub use session::SessionGuard;

use crate::config::AppConfig;
use smr_protocol::{extract_tool_call_texts, ExtractedText, TextPointer};

pub struct DlpEngine {
    content: ContentDlp,
    file: FileDlp,
    sessions: SessionGuard,
    enabled: bool,
}

impl DlpEngine {
    pub fn new(config: &AppConfig) -> anyhow::Result<Self> {
        let enabled = config.pipeline.dlp_active();
        Ok(Self {
            content: ContentDlp::new(&config.content_rules, &config.pipeline)?,
            file: FileDlp::new(&config.file_rules)?,
            sessions: SessionGuard::new(),
            enabled,
        })
    }

    pub fn reload(&self, config: &AppConfig) -> anyhow::Result<()> {
        self.file.reload(&config.file_rules)
    }

    pub fn is_file_index_ready(&self) -> bool {
        self.file.is_index_ready()
    }

    pub fn process_request(
        &self,
        session_id: &str,
        extracted: &[ExtractedText],
        request_json: &serde_json::Value,
    ) -> anyhow::Result<(Vec<(ExtractedText, String)>, usize)> {
        if !self.enabled {
            return Ok((Vec::new(), 0));
        }

        let tool_texts = extract_tool_call_texts(request_json)?;
        let tool_blob: String = tool_texts.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join("\n");

        if !tool_blob.is_empty() {
            self.file
                .check_path_triggers_in_tool_text(session_id, &tool_blob, |sid, rule, contents| {
                    self.sessions.activate(sid, rule, contents, rule.trigger_window);
                });
        }

        let mut replacements = Vec::new();
        for item in extracted {
            let sanitized = self.content.sanitize_text(&item.text)?;
            let sanitized = self.sessions.sanitize_with_session(session_id, &sanitized)?;
            if sanitized != item.text {
                replacements.push((item.clone(), sanitized));
            }
        }
        let count = replacements.len();
        Ok((replacements, count))
    }

    /// Response-side: activate SessionGuard when model tool_calls reference protected paths.
    pub fn process_response_triggers(
        &self,
        session_id: &str,
        response_json: &serde_json::Value,
    ) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let tool_texts = extract_tool_call_texts(response_json)?;
        let tool_blob: String = tool_texts.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join("\n");
        if !tool_blob.is_empty() {
            self.file
                .check_path_triggers_in_tool_text(session_id, &tool_blob, |sid, rule, contents| {
                    self.sessions.activate(sid, rule, contents, rule.trigger_window);
                });
        }
        Ok(())
    }

    pub fn is_tool_field(pointer: &TextPointer) -> bool {
        matches!(pointer, TextPointer::OpenAiToolCallArguments { .. })
    }
}
