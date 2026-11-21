//! Deterministic file-reader segment parser and chunker.
//!
//! Consumes the `file-engine` wire contract (`pages[{page_number, unit,
//! title, markdown, markdown_sha256}]`), verifies `markdown_sha256`, maps
//! `unit` to [`AnchorKind`], and splits each segment into overlapping chunks
//! with exact 0-based Unicode code-point offsets (`char_start..char_end`, end
//! exclusive) such that `chunk.text` equals that slice of `segment.markdown`
//! verbatim. Never chunks across segment boundaries.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{MakakooError, Result};

use super::{
    Anchor, AnchorKind, DEFAULT_CHUNK_MAX_CHARS, DEFAULT_CHUNK_MIN_CHARS,
    DEFAULT_CHUNK_OVERLAP_CHARS, DEFAULT_CHUNK_TARGET_CHARS,
};

/// Wire contract extractor descriptor from `file-engine`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEngineExtractor {
    pub name: String,
    pub version: String,
}

/// One extracted segment from `file-engine`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEngineSegment {
    pub page_number: u64,
    pub unit: String,
    #[serde(default)]
    pub title: Option<String>,
    pub markdown: String,
    pub markdown_sha256: String,
}

/// Top-level `file-engine` wire output (v1/v2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEngineOutput {
    pub contract: String,
    #[serde(default)]
    pub extractor: Option<FileEngineExtractor>,
    pub format: String,
    pub pages: Vec<FileEngineSegment>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Chunking parameters in Unicode code points.
///
/// Defaults (`min = 800`, `target = 1000`, `max = 1200`, `overlap = 100`)
/// split at paragraph boundaries first, then sentence boundaries, then
/// whitespace, and never cross segment boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkOptions {
    pub min_chars: usize,
    pub target_chars: usize,
    pub max_chars: usize,
    pub overlap_chars: usize,
}

impl Default for ChunkOptions {
    fn default() -> Self {
        Self {
            min_chars: DEFAULT_CHUNK_MIN_CHARS,
            target_chars: DEFAULT_CHUNK_TARGET_CHARS,
            max_chars: DEFAULT_CHUNK_MAX_CHARS,
            overlap_chars: DEFAULT_CHUNK_OVERLAP_CHARS,
        }
    }
}

impl ChunkOptions {
    pub fn validate(&self) -> Result<()> {
        if self.max_chars == 0 {
            return Err(MakakooError::invalid_input("max_chars must be > 0"));
        }
        if self.min_chars > self.max_chars {
            return Err(MakakooError::invalid_input(format!(
                "min_chars ({}) must be <= max_chars ({})",
                self.min_chars, self.max_chars
            )));
        }
        if self.target_chars < self.min_chars || self.target_chars > self.max_chars {
            return Err(MakakooError::invalid_input(format!(
                "target_chars ({}) must be within min_chars..=max_chars ({}..={})",
                self.target_chars, self.min_chars, self.max_chars
            )));
        }
        if self.overlap_chars >= self.max_chars {
            return Err(MakakooError::invalid_input(format!(
                "overlap_chars ({}) must be < max_chars ({})",
                self.overlap_chars, self.max_chars
            )));
        }
        Ok(())
    }
}

/// One chunk extracted from a single `FileEngineSegment`, ready for embedding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextChunk {
    pub anchor: Anchor,
    /// 0-based Unicode code-point start offset into `segment.markdown`.
    pub char_start: usize,
    /// 0-based Unicode code-point end offset (exclusive) into `segment.markdown`.
    pub char_end: usize,
    /// Exact verbatim slice of `segment.markdown` for `char_start..char_end`.
    pub text: String,
    /// 0-based index of the originating segment in `FileEngineOutput::pages`.
    pub segment_index: usize,
}

/// Map a `file-engine` segment to its citation [`Anchor`].
pub fn anchor_from_segment(segment: &FileEngineSegment) -> Result<Anchor> {
    let trimmed_title = segment
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match segment.unit.as_str() {
        "page" => Ok(Anchor {
            kind: AnchorKind::Page,
            value: segment.page_number.to_string(),
        }),
        "sheet_part" | "sheet" => Ok(Anchor {
            kind: AnchorKind::Sheet,
            value: trimmed_title
                .map(str::to_string)
                .unwrap_or_else(|| segment.page_number.to_string()),
        }),
        "slide" => Ok(Anchor {
            kind: AnchorKind::Slide,
            value: segment.page_number.to_string(),
        }),
        "section" | "heading" => Ok(Anchor {
            kind: AnchorKind::Heading,
            value: trimmed_title
                .map(str::to_string)
                .unwrap_or_else(|| format!("Section {}", segment.page_number)),
        }),
        other => Err(MakakooError::invalid_input(format!(
            "unsupported file-engine segment unit: {other:?}"
        ))),
    }
}

/// Verify `segment.markdown_sha256` against the SHA-256 of `segment.markdown`.
pub fn verify_segment_sha256(segment: &FileEngineSegment) -> Result<()> {
    let actual = hex::encode(Sha256::digest(segment.markdown.as_bytes()));
    if actual != segment.markdown_sha256.trim().to_ascii_lowercase() {
        return Err(MakakooError::invalid_input(format!(
            "markdown_sha256 mismatch on page_number {}: expected {}, got {}",
            segment.page_number, segment.markdown_sha256, actual
        )));
    }
    Ok(())
}

/// Parse and chunk every segment in a [`FileEngineOutput`].
///
/// Verifies `markdown_sha256` on every segment and never merges or splits
/// across segment boundaries.
pub fn chunk_file_engine_output(
    output: &FileEngineOutput,
    opts: &ChunkOptions,
) -> Result<Vec<TextChunk>> {
    opts.validate()?;
    let mut all = Vec::new();
    for (seg_idx, segment) in output.pages.iter().enumerate() {
        let mut seg_chunks = chunk_segment(segment, seg_idx, opts)?;
        all.append(&mut seg_chunks);
    }
    Ok(all)
}

/// Chunk one [`FileEngineSegment`] into verbatim code-point slices.
pub fn chunk_segment(
    segment: &FileEngineSegment,
    segment_index: usize,
    opts: &ChunkOptions,
) -> Result<Vec<TextChunk>> {
    opts.validate()?;
    verify_segment_sha256(segment)?;
    let anchor = anchor_from_segment(segment)?;

    if segment.markdown.is_empty() {
        return Ok(Vec::new());
    }

    let chars: Vec<char> = segment.markdown.chars().collect();
    let mut byte_offsets: Vec<usize> = Vec::with_capacity(chars.len() + 1);
    for (byte_idx, _) in segment.markdown.char_indices() {
        byte_offsets.push(byte_idx);
    }
    byte_offsets.push(segment.markdown.len());

    let total = chars.len();
    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < total {
        let remaining = total - start;
        let end = if remaining <= opts.max_chars {
            total
        } else {
            choose_split_end(&chars, start, opts)
        };

        let text = segment.markdown[byte_offsets[start]..byte_offsets[end]].to_string();
        chunks.push(TextChunk {
            anchor: anchor.clone(),
            char_start: start,
            char_end: end,
            text,
            segment_index,
        });

        if end >= total {
            break;
        }

        let next = choose_next_start(&chars, start, end, opts.overlap_chars);
        debug_assert!(next > start, "chunker must always advance");
        start = next;
    }

    Ok(chunks)
}

/// Pick a split boundary `end` in `[start + min_chars, start + max_chars]`
/// preferring (1) paragraph break `\n\n`, (2) sentence boundary (`.`, `!`, `?`
/// followed by whitespace), (3) single newline `\n`, (4) whitespace, else hard
/// split at `start + max_chars`.
fn choose_split_end(chars: &[char], start: usize, opts: &ChunkOptions) -> usize {
    let min_end = (start + opts.min_chars).min(chars.len());
    let target_end = (start + opts.target_chars).min(chars.len());
    let max_end = (start + opts.max_chars).min(chars.len());

    let best_closest = |candidates: &[usize]| -> Option<usize> {
        candidates
            .iter()
            .copied()
            .min_by_key(|&pos| (pos.abs_diff(target_end), std::cmp::Reverse(pos)))
    };

    // 1. Paragraph boundaries (`\n\n` — end right before `\n\n` so paragraphs stay intact).
    let mut para = Vec::new();
    for pos in min_end..=max_end {
        if pos + 1 < chars.len() && chars[pos] == '\n' && chars[pos + 1] == '\n' {
            para.push(pos);
        }
    }
    if let Some(pos) = best_closest(&para) {
        return pos;
    }

    // 2. Sentence boundaries (`.` / `!` / `?` followed by whitespace or end).
    let mut sent = Vec::new();
    for pos in min_end..=max_end {
        if pos > start {
            let prev = chars[pos - 1];
            let next_is_ws = pos == chars.len() || chars[pos].is_whitespace();
            if matches!(prev, '.' | '!' | '?') && next_is_ws {
                sent.push(pos);
            }
        }
    }
    if let Some(pos) = best_closest(&sent) {
        return pos;
    }

    // 3. Line breaks.
    let mut lines = Vec::new();
    for pos in min_end..=max_end {
        if pos < chars.len() && chars[pos] == '\n' {
            lines.push(pos);
        }
    }
    if let Some(pos) = best_closest(&lines) {
        return pos;
    }

    // 4. Word / whitespace boundaries.
    let mut ws = Vec::new();
    for pos in min_end..=max_end {
        if pos < chars.len() && chars[pos].is_whitespace() {
            ws.push(pos);
        }
    }
    if let Some(pos) = best_closest(&ws) {
        return pos;
    }

    max_end
}

/// Compute the next chunk start with ~`overlap_chars` overlap, snapped cleanly
/// onto a non-whitespace token start while strictly advancing past `start`.
fn choose_next_start(chars: &[char], start: usize, end: usize, overlap_chars: usize) -> usize {
    if overlap_chars == 0 {
        let mut s = end;
        while s < chars.len() && chars[s].is_whitespace() {
            s += 1;
        }
        return if s > start { s } else { end };
    }

    let raw = end.saturating_sub(overlap_chars).max(start + 1);
    // Advance past a partial word only if a whitespace break exists inside
    // the overlap window; otherwise start directly at `raw`.
    let mut candidate = raw;
    if candidate > 0 && !chars[candidate - 1].is_whitespace() && !chars[candidate].is_whitespace() {
        let mut scan = candidate;
        while scan < end && !chars[scan].is_whitespace() {
            scan += 1;
        }
        while scan < end && chars[scan].is_whitespace() {
            scan += 1;
        }
        if scan < end && scan > start {
            candidate = scan;
        }
    } else {
        while candidate < end && chars[candidate].is_whitespace() {
            candidate += 1;
        }
        if candidate >= end {
            candidate = raw;
        }
    }
    candidate.max(start + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_segment(
        page_number: u64,
        unit: &str,
        title: Option<&str>,
        markdown: &str,
    ) -> FileEngineSegment {
        let sha = hex::encode(Sha256::digest(markdown.as_bytes()));
        FileEngineSegment {
            page_number,
            unit: unit.to_string(),
            title: title.map(str::to_string),
            markdown: markdown.to_string(),
            markdown_sha256: sha,
        }
    }

    fn assert_exact_codepoint_slices(markdown: &str, chunks: &[TextChunk]) {
        assert!(!chunks.is_empty(), "expected at least one chunk");
        let total = markdown.chars().count();
        for c in chunks {
            assert!(c.char_start < c.char_end);
            assert!(c.char_end <= total);
            let expected: String = markdown
                .chars()
                .skip(c.char_start)
                .take(c.char_end - c.char_start)
                .collect();
            assert_eq!(
                c.text, expected,
                "chunk text must equal verbatim code-point slice {}..{}",
                c.char_start, c.char_end
            );
        }
    }

    #[test]
    fn rejects_markdown_sha256_mismatch() {
        let mut seg = make_segment(1, "page", None, "Authentic content.");
        seg.markdown_sha256 = "0".repeat(64);
        let err = chunk_segment(&seg, 0, &ChunkOptions::default()).unwrap_err();
        assert!(matches!(err, MakakooError::InvalidInput(_)));
    }

    #[test]
    fn rejects_unknown_segment_unit() {
        let seg = make_segment(1, "audio_frame", None, "Some text.");
        let err = chunk_segment(&seg, 0, &ChunkOptions::default()).unwrap_err();
        assert!(matches!(err, MakakooError::InvalidInput(_)));
    }

    #[test]
    fn exact_unicode_slice_for_page_anchor() {
        let md = "Página 4: cláusula de indemnización — prüfen wir jährlich.\n\n".repeat(40);
        let seg = make_segment(4, "page", None, &md);
        let chunks = chunk_segment(&seg, 0, &ChunkOptions::default()).unwrap();
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].anchor.kind, AnchorKind::Page);
        assert_eq!(chunks[0].anchor.value, "4");
        assert_exact_codepoint_slices(&md, &chunks);
    }

    #[test]
    fn exact_unicode_slice_for_sheet_anchor() {
        let md = "| Konto | Betrag € | Währung |\n| --- | --- | --- |\n| Żółć | 120,50 € | EUR |\n"
            .repeat(30);
        let seg = make_segment(2, "sheet_part", Some("Q4 Budget"), &md);
        let chunks = chunk_segment(&seg, 0, &ChunkOptions::default()).unwrap();
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].anchor.kind, AnchorKind::Sheet);
        assert_eq!(chunks[0].anchor.value, "Q4 Budget");
        assert_exact_codepoint_slices(&md, &chunks);
    }

    #[test]
    fn exact_unicode_slice_for_slide_anchor() {
        let md = "Slide 7 — Resumen estratégico: acción, diseño, y métricas 🚀. ".repeat(35);
        let seg = make_segment(7, "slide", Some("Strategy"), &md);
        let chunks = chunk_segment(&seg, 0, &ChunkOptions::default()).unwrap();
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].anchor.kind, AnchorKind::Slide);
        assert_eq!(chunks[0].anchor.value, "7");
        assert_exact_codepoint_slices(&md, &chunks);
    }

    #[test]
    fn exact_unicode_slice_for_heading_anchor() {
        let md = "Übersicht über die Anforderungen und Prüfungen für Kunden. ".repeat(35);
        let seg = make_segment(1, "section", Some("2.1 Datenschutz"), &md);
        let chunks = chunk_segment(&seg, 0, &ChunkOptions::default()).unwrap();
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].anchor.kind, AnchorKind::Heading);
        assert_eq!(chunks[0].anchor.value, "2.1 Datenschutz");
        assert_exact_codepoint_slices(&md, &chunks);
    }

    #[test]
    fn never_chunks_across_segments_and_respects_paragraph_and_overlap() {
        let p1 = "A".repeat(450) + ".\n\n" + &"B".repeat(480) + ".\n\n" + &"C".repeat(450) + ".";
        let p2 = "Second segment short text.";
        let output = FileEngineOutput {
            contract: "v1".to_string(),
            extractor: Some(FileEngineExtractor {
                name: "file-engine-office".to_string(),
                version: "1.0.0".to_string(),
            }),
            format: "docx".to_string(),
            pages: vec![
                make_segment(1, "section", Some("Intro"), &p1),
                make_segment(2, "section", Some("Next"), p2),
            ],
            warnings: vec![],
        };

        let chunks = chunk_file_engine_output(&output, &ChunkOptions::default()).unwrap();
        let seg0: Vec<&TextChunk> = chunks.iter().filter(|c| c.segment_index == 0).collect();
        let seg1: Vec<&TextChunk> = chunks.iter().filter(|c| c.segment_index == 1).collect();

        assert_eq!(seg0.len(), 2);
        assert_eq!(seg1.len(), 1);
        assert_eq!(seg1[0].text, p2);
        assert_eq!(seg1[0].char_start, 0);
        assert_eq!(seg1[0].char_end, p2.chars().count());
        // First chunk ended on the paragraph boundary (~933 chars, inside 800..=1200).
        let first_len = seg0[0].char_end - seg0[0].char_start;
        assert!((800..=1200).contains(&first_len), "len={first_len}");
        // Second chunk overlaps the tail of the first by ~100 chars.
        assert!(seg0[1].char_start < seg0[0].char_end);
        let overlap = seg0[0].char_end - seg0[1].char_start;
        assert!((50..=100).contains(&overlap), "overlap={overlap}");
    }
}
