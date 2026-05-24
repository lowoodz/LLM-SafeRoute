use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aho_corasick::AhoCorasick;
use anyhow::Result;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::RwLock;

use crate::config::FileRule;
use crate::dlp::file::{load_rule_contents, FileContent};

pub const CHUNK_SIZE: usize = 8192;

#[derive(Clone)]
pub struct FileChunk {
    pub source: PathBuf,
    pub chunk_index: usize,
    pub text: String,
}

#[derive(Clone)]
pub struct IndexedRule {
    pub rule: FileRule,
    pub normalized_path: String,
    pub contents: Vec<FileContent>,
    pub chunks: Vec<FileChunk>,
}

struct FileIndexState {
    rules: Vec<IndexedRule>,
    automaton: Option<AhoCorasick>,
    needle_map: Vec<(usize, usize, usize)>,
    ready: bool,
}

pub struct FileIndexManager {
    inner: Arc<RwLock<FileIndexState>>,
}

impl FileIndexManager {
    pub fn new(rules: &[FileRule]) -> Self {
        let inner = Arc::new(RwLock::new(FileIndexState {
            rules: Vec::new(),
            automaton: None,
            needle_map: Vec::new(),
            ready: false,
        }));
        let mgr = Self { inner: inner.clone() };
        let rules_vec = rules.to_vec();
        std::thread::spawn(move || {
            if let Ok(state) = build_index(&rules_vec) {
                *inner.write() = state;
            }
        });
        mgr.spawn_watcher(rules);
        mgr
    }

    pub fn is_ready(&self) -> bool {
        self.inner.read().ready
    }

    pub fn rules(&self) -> Vec<IndexedRule> {
        self.inner.read().rules.clone()
    }

    pub fn rebuild_sync(&self, rules: &[FileRule]) -> Result<()> {
        *self.inner.write() = build_index(rules)?;
        Ok(())
    }

    fn spawn_watcher(&self, rules: &[FileRule]) {
        let paths: Vec<PathBuf> = rules
            .iter()
            .filter(|r| r.enabled)
            .map(|r| r.path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let inner = self.inner.clone();
        let rules_owned = rules.to_vec();
        std::thread::spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let Ok(mut watcher) = RecommendedWatcher::new(
                move |res: Result<notify::Event, notify::Error>| {
                    if let Ok(event) = res {
                        if matches!(
                            event.kind,
                            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                        ) {
                            let _ = tx.send(());
                        }
                    }
                },
                Config::default(),
            ) else {
                return;
            };
            for p in &paths {
                let mode = if p.is_dir() {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                let _ = watcher.watch(p, mode);
            }
            while rx.recv().is_ok() {
                std::thread::sleep(Duration::from_millis(500));
                if let Ok(state) = build_index(&rules_owned) {
                    *inner.write() = state;
                }
            }
        });
    }
}

fn build_index(rules: &[FileRule]) -> Result<FileIndexState> {
    let mut indexed_rules = Vec::new();
    let mut patterns: Vec<String> = Vec::new();
    let mut needle_map: Vec<(usize, usize, usize)> = Vec::new();

    for (ri, rule) in rules.iter().filter(|r| r.enabled).enumerate() {
        let contents = load_rule_contents(rule)?;
        let normalized_path = rule.path.to_string_lossy().replace('\\', "/");
        let mut chunks = Vec::new();
        for (ci, file) in contents.iter().enumerate() {
            if file.text.is_empty() {
                continue;
            }
            if file.text.len() <= CHUNK_SIZE * 2 {
                patterns.push(file.text.clone());
                needle_map.push((ri, ci, 0));
            } else {
                for (idx, chunk) in file.text.as_bytes().chunks(CHUNK_SIZE).enumerate() {
                    let text = String::from_utf8_lossy(chunk).into_owned();
                    if text.len() >= 4 {
                        patterns.push(text.clone());
                        needle_map.push((ri, ci, idx));
                        chunks.push(FileChunk {
                            source: file.source.clone(),
                            chunk_index: idx,
                            text,
                        });
                    }
                }
            }
        }
        indexed_rules.push(IndexedRule {
            rule: rule.clone(),
            normalized_path,
            contents,
            chunks,
        });
    }

    let automaton = if patterns.is_empty() {
        None
    } else {
        Some(AhoCorasick::new(&patterns)?)
    };

    Ok(FileIndexState {
        rules: indexed_rules,
        automaton,
        needle_map,
        ready: true,
    })
}
