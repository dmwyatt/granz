use std::fmt;

use sha2::{Digest, Sha256};

/// Tag for display/filtering. New sources add a variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkSourceType {
    TranscriptWindow,
    PanelSection,
    NotesParagraph,
}

impl ChunkSourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChunkSourceType::TranscriptWindow => "transcript_window",
            ChunkSourceType::PanelSection => "panel_section",
            ChunkSourceType::NotesParagraph => "notes_paragraph",
        }
    }

    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "transcript_window" => Some(ChunkSourceType::TranscriptWindow),
            "panel_section" => Some(ChunkSourceType::PanelSection),
            "notes_paragraph" => Some(ChunkSourceType::NotesParagraph),
            _ => None,
        }
    }
}

impl fmt::Display for ChunkSourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A unit of text to embed. The pipeline doesn't care where this came from.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub source_type: ChunkSourceType,
    pub source_id: String,
    pub document_id: String,
    /// The raw chunk text: stored in the db and shown in search snippets.
    pub text: String,
    /// Hash of the full embed input (header included when present), so
    /// header changes re-embed via the normal diff.
    pub content_hash: String,
    pub metadata: Option<serde_json::Value>,
    /// Contextual header prepended to the embed input only; never stored
    /// as chunk text and never displayed.
    pub header: Option<String>,
}

impl Chunk {
    /// The text actually sent to the embedding model.
    pub fn embed_input(&self) -> std::borrow::Cow<'_, str> {
        match &self.header {
            Some(header) => std::borrow::Cow::Owned(format!("{}{}", header, self.text)),
            None => std::borrow::Cow::Borrowed(&self.text),
        }
    }
}

/// Compute SHA-256 hash of text content.
pub fn hash_content(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Hash the full embed input (header + text when a header is present), so
/// chunks re-embed when either part changes.
pub fn hash_embed_input(header: Option<&str>, text: &str) -> String {
    match header {
        Some(h) => hash_content(&format!("{}{}", h, text)),
        None => hash_content(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_content_deterministic() {
        let h1 = hash_content("hello world");
        let h2 = hash_content("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_content_different_inputs() {
        let h1 = hash_content("hello");
        let h2 = hash_content("world");
        assert_ne!(h1, h2);
    }

    fn chunk_with_header(header: Option<&str>) -> Chunk {
        Chunk {
            source_type: ChunkSourceType::TranscriptWindow,
            source_id: "doc1:c0".to_string(),
            document_id: "doc1".to_string(),
            text: "the chunk body".to_string(),
            content_hash: hash_content("the chunk body"),
            metadata: None,
            header: header.map(|h| h.to_string()),
        }
    }

    #[test]
    fn test_embed_input_without_header_is_text() {
        let chunk = chunk_with_header(None);
        assert_eq!(chunk.embed_input(), "the chunk body");
    }

    #[test]
    fn test_embed_input_with_header_prepends() {
        let chunk = chunk_with_header(Some("Meeting: Sync\nDate: 2026-07-01\n\n"));
        assert_eq!(
            chunk.embed_input(),
            "Meeting: Sync\nDate: 2026-07-01\n\nthe chunk body"
        );
    }

    #[test]
    fn test_chunk_source_type_roundtrip() {
        let t = ChunkSourceType::TranscriptWindow;
        assert_eq!(ChunkSourceType::from_str(t.as_str()), Some(t));

        let t = ChunkSourceType::PanelSection;
        assert_eq!(ChunkSourceType::from_str(t.as_str()), Some(t));

        let t = ChunkSourceType::NotesParagraph;
        assert_eq!(ChunkSourceType::from_str(t.as_str()), Some(t));
    }

    #[test]
    fn test_chunk_source_type_unknown() {
        assert_eq!(ChunkSourceType::from_str("unknown"), None);
    }
}
