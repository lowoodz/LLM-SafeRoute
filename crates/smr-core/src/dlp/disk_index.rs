//! Disk-backed file index: SQLite signatures + Bloom pre-filter (P0 large corpus).

use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;
use std::thread;

use anyhow::Result;
use parking_lot::RwLock;
use rusqlite::{params, Connection};
use walkdir::WalkDir;
use xxhash_rust::xxh64::xxh64;

use crate::config::{FileRule, MatchMode};
use crate::dlp::bloom::BloomFilter;
use crate::dlp::doc_extract;
use crate::dlp::file::path_trigger_match;
use crate::dlp::fragment::effective_min_fragment_len;
use crate::dlp::sanitize::{sanitize_range, sanitize_whole};
use crate::dlp::session::ActiveFileContent;
use crate::paths;

pub const INDEX_READ_CAP: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct IndexedRule {
    pub rule: FileRule,
    pub normalized_path: String,
}

struct RuleSnapshot {
    generation: u64,
    bloom: BloomFilter,
    db_path: PathBuf,
}

struct IndexState {
    ready: AtomicBool,
    rules: Vec<IndexedRule>,
    snapshots: std::collections::HashMap<String, Arc<RuleSnapshot>>,
}

pub struct FileIndexManager {
    inner: Arc<RwLock<IndexState>>,
    index_root: PathBuf,
}

struct SigRow {
    hash: u64,
    path: String,
    byte_offset: u64,
    byte_len: u32,
}

impl FileIndexManager {
    pub fn new(rules: &[FileRule]) -> Self {
        let index_root = paths::config_dir().join("file-index");
        let inner = Arc::new(RwLock::new(IndexState {
            ready: AtomicBool::new(false),
            rules: Vec::new(),
            snapshots: std::collections::HashMap::new(),
        }));
        let mgr = Self {
            inner: inner.clone(),
            index_root: index_root.clone(),
        };
        let rules_vec = rules.to_vec();
        thread::spawn(move || {
            if let Ok(state) = build_all_rules(&index_root, &rules_vec) {
                let mut guard = inner.write();
                guard.ready.store(true, Ordering::Release);
                guard.rules = state.rules;
                guard.snapshots = state.snapshots;
            } else {
                tracing::error!("file index build failed");
            }
        });
        mgr.spawn_watcher(rules);
        mgr
    }

    pub fn is_ready(&self) -> bool {
        self.inner.read().ready.load(Ordering::Acquire)
    }

    pub fn rules(&self) -> Vec<IndexedRule> {
        self.inner.read().rules.clone()
    }

    pub fn rebuild_sync(&self, rules: &[FileRule]) -> Result<()> {
        self.inner.write().ready.store(false, Ordering::Release);
        let state = build_all_rules(&self.index_root, rules)?;
        let mut guard = self.inner.write();
        guard.ready.store(true, Ordering::Release);
        guard.rules = state.rules;
        guard.snapshots = state.snapshots;
        Ok(())
    }

    pub fn scan_and_sanitize(&self, text: &str, active: &[ActiveFileContent]) -> String {
        let guard = self.inner.read();
        if !guard.ready.load(Ordering::Acquire) {
            return text.to_string();
        }
        let mut result = text.to_string();
        for item in active {
            let Some(snapshot) = guard.snapshots.get(&item.rule.id) else {
                continue;
            };
            result = scan_haystack(&result, &item.rule, snapshot);
        }
        result
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
        let index_root = self.index_root.clone();
        let rules_owned = rules.to_vec();
        thread::spawn(move || {
            use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
            let (tx, rx) = mpsc::channel();
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
                thread::sleep(std::time::Duration::from_millis(2000));
                if let Ok(state) = build_all_rules(&index_root, &rules_owned) {
                    let mut guard = inner.write();
                    guard.ready.store(true, Ordering::Release);
                    guard.rules = state.rules;
                    guard.snapshots = state.snapshots;
                }
            }
        });
    }
}

struct BuiltState {
    rules: Vec<IndexedRule>,
    snapshots: std::collections::HashMap<String, Arc<RuleSnapshot>>,
}

fn build_all_rules(index_root: &Path, rules: &[FileRule]) -> Result<BuiltState> {
    fs::create_dir_all(index_root)?;
    let mut indexed_rules = Vec::new();
    let mut snapshots = std::collections::HashMap::new();
    for rule in rules.iter().filter(|r| r.enabled) {
        let normalized_path = rule.path.to_string_lossy().replace('\\', "/");
        indexed_rules.push(IndexedRule {
            rule: rule.clone(),
            normalized_path: normalized_path.clone(),
        });
        let snapshot = build_rule_index(index_root, rule)?;
        snapshots.insert(rule.id.clone(), Arc::new(snapshot));
    }
    Ok(BuiltState {
        rules: indexed_rules,
        snapshots,
    })
}

fn build_rule_index(index_root: &Path, rule: &FileRule) -> Result<RuleSnapshot> {
    let rule_dir = index_root.join(sanitize_id(&rule.id));
    if rule_dir.exists() {
        fs::remove_dir_all(&rule_dir).ok();
    }
    fs::create_dir_all(&rule_dir)?;
    let db_path = rule_dir.join("index.db");
    let bloom_path = rule_dir.join("bloom.bin");
    let generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1);

    let bit_count = rule.index.bloom_megabytes * 1024 * 1024 * 8;
    let mut bloom = BloomFilter::new(bit_count.max(1 << 16));
    let conn = Connection::open(&db_path)?;
    conn.execute_batch(
        "CREATE TABLE signatures (
            sig_hash INTEGER NOT NULL,
            path TEXT NOT NULL,
            byte_offset INTEGER NOT NULL,
            byte_len INTEGER NOT NULL
        );
        CREATE INDEX idx_sig_hash ON signatures(sig_hash);",
    )?;
    drop(conn);

    let files = collect_files(rule)?;
    let file_count = files.len();
    let workers = rule.index.build_workers.max(1).min(16);
    let (batch_tx, batch_rx) = mpsc::sync_channel::<Vec<SigRow>>(workers * 4);
    let db_path_writer = db_path.clone();
    let files_arc = Arc::new(files);

    let writer = thread::spawn(move || -> Result<u64> {
        let conn = Connection::open(&db_path_writer)?;
        let mut count = 0u64;
        while let Ok(batch) = batch_rx.recv() {
            if batch.is_empty() {
                break;
            }
            let tx = conn.unchecked_transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO signatures (sig_hash, path, byte_offset, byte_len) VALUES (?1, ?2, ?3, ?4)",
                )?;
                for row in &batch {
                    stmt.execute(params![
                        row.hash as i64,
                        row.path,
                        row.byte_offset as i64,
                        row.byte_len as i64
                    ])?;
                    count += 1;
                }
            }
            tx.commit()?;
        }
        Ok(count)
    });

    let mut file_workers = Vec::new();
    for worker_id in 0..workers {
        let files_w = files_arc.clone();
        let batch_tx = batch_tx.clone();
        let rule_w = rule.clone();
        file_workers.push(thread::spawn(move || {
            let mut idx = worker_id;
            while idx < files_w.len() {
                let path = &files_w[idx];
                if let Err(e) = index_one_file(path, &rule_w, &batch_tx) {
                    tracing::warn!(path = %path.display(), error = %e, "index file failed");
                }
                idx += workers;
            }
        }));
    }
    let shutdown_tx = batch_tx.clone();
    drop(batch_tx);
    for h in file_workers {
        let _ = h.join();
    }
    let _ = shutdown_tx.send(Vec::new());
    let sig_count = writer.join().unwrap_or(Ok(0))?;

    // Build bloom from DB (streaming read hashes)
    {
        let conn = Connection::open(&db_path)?;
        let mut stmt = conn.prepare("SELECT sig_hash FROM signatures")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let h: i64 = row.get(0)?;
            bloom.insert_hash(h as u64);
        }
    }
    bloom.save(&bloom_path)?;

    let manifest = rule_dir.join("manifest.json");
    let mut mf = File::create(&manifest)?;
    write!(
        mf,
        r#"{{"generation":{generation},"signatures":{sig_count},"files":{file_count}}}"#
    )?;

    tracing::info!(
        rule_id = %rule.id,
        signatures = sig_count,
        "file index built (disk-backed)"
    );

    Ok(RuleSnapshot {
        generation,
        bloom,
        db_path,
    })
}

fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn collect_files(rule: &FileRule) -> Result<Vec<PathBuf>> {
    let path = &rule.path;
    if !path.exists() {
        tracing::warn!(path = %path.display(), "file rule path does not exist");
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    if path.is_file() {
        out.push(path.clone());
        return Ok(out);
    }
    let walker = if rule.recursive {
        WalkDir::new(path).into_iter()
    } else {
        WalkDir::new(path).max_depth(1).into_iter()
    };
    for entry in walker.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.is_file() && matches_format(p, &rule.formats) {
            out.push(p.to_path_buf());
        }
    }
    Ok(out)
}

fn matches_format(path: &Path, formats: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| formats.iter().any(|f| f.eq_ignore_ascii_case(ext)))
        .unwrap_or(false)
}

fn index_one_file(path: &Path, rule: &FileRule, tx: &SyncSender<Vec<SigRow>>) -> Result<()> {
    let meta = fs::metadata(path)?;
    let size = meta.len();
    let path_str = path.to_string_lossy().replace('\\', "/");

    if size <= rule.index.max_full_file_bytes && rule.match_mode == MatchMode::Full {
        let data = read_file_bytes(path, size)?;
        if data.is_empty() {
            return Ok(());
        }
        let mut rows = Vec::new();
        push_chunk_signatures(&data, 0, &path_str, rule, &mut rows);
        if !rows.is_empty() {
            let _ = tx.send(rows);
        }
        return Ok(());
    }

    if size <= INDEX_READ_CAP {
        if let Some(text) = doc_extract::extract_text(path).ok() {
            if !text.is_empty() {
                let bytes = text.into_bytes();
                let mut rows = Vec::new();
                index_bytes_chunks(&bytes, 0, &path_str, rule, &mut rows);
                if !rows.is_empty() {
                    let _ = tx.send(rows);
                }
                return Ok(());
            }
        }
        let data = read_file_bytes(path, size)?;
        let mut rows = Vec::new();
        index_bytes_chunks(&data, 0, &path_str, rule, &mut rows);
        if !rows.is_empty() {
            let _ = tx.send(rows);
        }
        return Ok(());
    }

    stream_index_file(path, &path_str, rule, tx)
}

fn read_file_bytes(path: &Path, size: u64) -> Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

fn stream_index_file(path: &Path, path_str: &str, rule: &FileRule, tx: &SyncSender<Vec<SigRow>>) -> Result<()> {
    let mut f = File::open(path)?;
    let chunk_size = rule.index.chunk_size;
    let overlap = rule.index.chunk_overlap;
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut carry = Vec::new();
    let mut file_offset: u64 = 0;
    let mut read_buf = vec![0u8; 1024 * 1024];
    let mut batch = Vec::new();

    loop {
        let n = f.read(&mut read_buf)?;
        if n == 0 {
            break;
        }
        carry.extend_from_slice(&read_buf[..n]);
        while carry.len() >= chunk_size {
            push_chunk_signatures(&carry[..chunk_size], file_offset, path_str, rule, &mut batch);
            if batch.len() >= 5000 {
                let _ = tx.send(std::mem::take(&mut batch));
            }
            if carry.len() <= step {
                carry.clear();
                file_offset += step as u64;
                break;
            }
            carry.drain(..step);
            file_offset += step as u64;
        }
    }
    if !carry.is_empty() {
        push_chunk_signatures(&carry, file_offset, path_str, rule, &mut batch);
    }
    if !batch.is_empty() {
        let _ = tx.send(batch);
    }
    Ok(())
}

fn index_bytes_chunks(data: &[u8], base_offset: u64, path_str: &str, rule: &FileRule, rows: &mut Vec<SigRow>) {
    let chunk_size = rule.index.chunk_size;
    let overlap = rule.index.chunk_overlap;
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut offset = 0usize;
    while offset < data.len() {
        let end = (offset + chunk_size).min(data.len());
        push_chunk_signatures(&data[offset..end], base_offset + offset as u64, path_str, rule, rows);
        if end >= data.len() {
            break;
        }
        offset += step;
    }
}

fn push_chunk_signatures(
    chunk: &[u8],
    byte_offset: u64,
    path: &str,
    rule: &FileRule,
    rows: &mut Vec<SigRow>,
) {
    if chunk.len() < 4 {
        return;
    }
    let h0 = xxh64(chunk, 0);
    rows.push(SigRow {
        hash: h0,
        path: path.to_string(),
        byte_offset,
        byte_len: chunk.len() as u32,
    });

    let min_len = effective_min_fragment_len(
        chunk.len(),
        rule.min_fragment_len,
        rule.min_fragment_ratio,
    )
    .max(4)
    .min(chunk.len());

    let stride = rule.index.signature_stride.max(1);
    let max_sigs = rule.index.signatures_per_chunk;
    let lens = signature_lengths(min_len, chunk.len());

    let mut emitted = 0usize;
    let mut pos = 0usize;
    while pos + min_len <= chunk.len() && emitted < max_sigs {
        for &len in &lens {
            if pos + len > chunk.len() {
                continue;
            }
            let slice = &chunk[pos..pos + len];
            rows.push(SigRow {
                hash: xxh64(slice, 0),
                path: path.to_string(),
                byte_offset: byte_offset + pos as u64,
                byte_len: len as u32,
            });
            emitted += 1;
            if emitted >= max_sigs {
                break;
            }
        }
        pos += stride;
    }
}

fn signature_lengths(min_len: usize, chunk_len: usize) -> Vec<usize> {
    let mut lens = vec![min_len];
    for &candidate in &[64usize, 256, 1024, 8192] {
        if candidate >= min_len && candidate <= chunk_len && !lens.contains(&candidate) {
            lens.push(candidate);
        }
    }
    if chunk_len > min_len && !lens.contains(&chunk_len) {
        lens.push(chunk_len);
    }
    lens
}

fn scan_haystack(haystack: &str, rule: &FileRule, snapshot: &RuleSnapshot) -> String {
    if haystack.is_empty() {
        return haystack.to_string();
    }
    if haystack.len() > rule.index.max_haystack_bytes {
        tracing::warn!(
            len = haystack.len(),
            max = rule.index.max_haystack_bytes,
            "haystack exceeds max_haystack_bytes; truncating scan"
        );
    }
    let scan_len = haystack.len().min(rule.index.max_haystack_bytes);
    let hay = &haystack[..scan_len];
    let bytes = hay.as_bytes();

    let min_len = effective_min_fragment_len(
        rule.index.chunk_size,
        rule.min_fragment_len,
        rule.min_fragment_ratio,
    )
    .max(4);

    let lens = signature_lengths(min_len, bytes.len().min(rule.index.chunk_size));

    let conn = match Connection::open_with_flags(
        &snapshot.db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(c) => c,
        Err(_) => return haystack.to_string(),
    };

    let mut byte_ranges: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        for &len in &lens {
            if pos + len > bytes.len() {
                continue;
            }
            let candidate = &bytes[pos..pos + len];
            let h = xxh64(candidate, 0);
            if !snapshot.bloom.may_contain(h) {
                continue;
            }
            if verify_candidate(&conn, h, candidate) {
                byte_ranges.push((pos, pos + len));
            }
        }
        pos += 1;
    }

    if byte_ranges.is_empty() {
        return haystack.to_string();
    }

    byte_ranges.sort_by_key(|r| r.0);
    let merged = merge_byte_ranges(byte_ranges);
    apply_byte_ranges(haystack, &merged, rule.match_mode)
}

fn verify_candidate(conn: &Connection, hash: u64, candidate: &[u8]) -> bool {
    let mut stmt = match conn.prepare(
        "SELECT path, byte_offset, byte_len FROM signatures WHERE sig_hash = ?1 LIMIT 16",
    ) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let mut rows = match stmt.query(params![hash as i64]) {
        Ok(r) => r,
        Err(_) => return false,
    };
    while let Ok(Some(row)) = rows.next() {
        let path: String = row.get(0).unwrap_or_default();
        let offset: i64 = row.get(1).unwrap_or(0);
        let len: i32 = row.get(2).unwrap_or(0);
        if len <= 0 {
            continue;
        }
        if let Ok(mut f) = File::open(&path) {
            let mut buf = vec![0u8; len as usize];
            if f.seek(std::io::SeekFrom::Start(offset as u64)).is_ok()
                && f.read_exact(&mut buf).is_ok()
                && buf == candidate
            {
                return true;
            }
        }
    }
    false
}

fn merge_byte_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| r.0);
    let mut out = vec![ranges[0]];
    for (s, e) in ranges.into_iter().skip(1) {
        let last = out.last_mut().unwrap();
        if s <= last.1 {
            last.1 = last.1.max(e);
        } else {
            out.push((s, e));
        }
    }
    out
}

fn apply_byte_ranges(text: &str, ranges: &[(usize, usize)], mode: MatchMode) -> String {
    let mut char_ranges: Vec<(usize, usize)> = Vec::new();
    for &(b_start, b_end) in ranges {
        if b_start >= text.len() || b_end > text.len() || b_start >= b_end {
            continue;
        }
        let prefix = &text[..b_start];
        let slice = &text[b_start..b_end];
        let cs = prefix.chars().count();
        let ce = cs + slice.chars().count();
        char_ranges.push((cs, ce));
    }
    char_ranges.sort_by_key(|r| r.0);
    let merged = merge_char_ranges(char_ranges);
    let mut result = text.to_string();
    for (start, end) in merged.into_iter().rev() {
        if mode == MatchMode::Full {
            let matched: String = result.chars().skip(start).take(end - start).collect();
            let rep = sanitize_whole(&matched);
            result = replace_char_range(&result, start, end, &rep);
        } else {
            result = sanitize_range(&result, start, end);
        }
    }
    result
}

fn merge_char_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| r.0);
    let mut out = vec![ranges[0]];
    for (s, e) in ranges.into_iter().skip(1) {
        let last = out.last_mut().unwrap();
        if s <= last.1 {
            last.1 = last.1.max(e);
        } else {
            out.push((s, e));
        }
    }
    out
}

fn replace_char_range(text: &str, start: usize, end: usize, replacement: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: String = chars.iter().take(start).collect();
    out.push_str(replacement);
    out.extend(chars.iter().skip(end));
    out
}

pub fn check_path_in_rules(rules: &[IndexedRule], tool_text: &str) -> Vec<FileRule> {
    rules
        .iter()
        .filter(|r| path_trigger_match(&r.normalized_path, tool_text))
        .map(|r| r.rule.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FileIndexOptions;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn builds_and_scans_small_dir() {
        let tmp = TempDir::new().unwrap();
        let secret = tmp.path().join("secret.txt");
        let mut f = File::create(&secret).unwrap();
        writeln!(f, "TOP-SECRET-PHRASE-XYZZY").unwrap();

        let rule = FileRule {
            id: "t1".into(),
            path: tmp.path().to_path_buf(),
            enabled: true,
            recursive: true,
            trigger_window: 3,
            match_mode: MatchMode::Fragment,
            min_fragment_len: Some(8),
            min_fragment_ratio: None,
            formats: vec!["txt".into()],
            index: FileIndexOptions {
                bloom_megabytes: 1,
                build_workers: 2,
                ..Default::default()
            },
        };

        let index_root = tmp.path().join("idx");
        let snapshot = build_rule_index(&index_root, &rule).unwrap();
        let hay = "user pasted TOP-SECRET-PHRASE-XYZZY here";
        let out = scan_haystack(hay, &rule, &snapshot);
        assert!(!out.contains("TOP-SECRET-PHRASE-XYZZY"));
        assert_eq!(out.chars().count(), hay.chars().count());
    }
}
