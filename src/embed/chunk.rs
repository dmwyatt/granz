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
    pub text: String,
    pub content_hash: String,
    pub metadata: Option<serde_json::Value>,
}

/// Compute SHA-256 hash of text content.
pub fn hash_content(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
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
