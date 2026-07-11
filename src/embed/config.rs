//! The embedding spec: chunking parameters plus the contextual-header
//! toggle. The spec is persisted in `embedding_metadata` alongside the
//! model name, so any binary can tell how a database's chunks were made
//! and keep embedding with that scheme instead of silently re-chunking
//! to its own compiled-in defaults.

use anyhow::{bail, Result};
use rusqlite::Connection;

use super::chunker::{ChunkingConfig, OverlapMode};
use super::store;

/// How the embeddings in a database are (or should be) built.
#[derive(Debug, Clone)]
pub struct EmbedSpec {
    pub chunking: ChunkingConfig,
    /// Prepend meeting title/date/attendees to the embed input.
    pub contextual_headers: bool,
}

/// Explicit overrides from `grans embed` experiment flags. `None` fields
/// leave the resolved value untouched.
#[derive(Debug, Clone, Default)]
pub struct EmbedOverrides {
    pub target_tokens: Option<usize>,
    pub overlap_tokens: Option<usize>,
    pub overlap_mode: Option<OverlapMode>,
    pub contextual_headers: Option<bool>,
}

impl EmbedSpec {
    /// The binary's default spec for a model with this token limit.
    pub fn default_for(max_tokens: usize) -> Self {
        Self {
            chunking: ChunkingConfig::from_max_length(max_tokens),
            contextual_headers: false,
        }
    }

    /// Resolve the spec for a database: stored params win over binary
    /// defaults, absent keys fall back to the defaults for `max_tokens`.
    /// Search, benchmark, and sync paths use this so a database embedded
    /// with a variant scheme is never silently migrated back.
    pub fn resolve_stored(conn: &Connection, max_tokens: usize) -> Self {
        let mut spec = Self::default_for(max_tokens);
        let stored = store::get_chunking_metadata(conn);
        if let Some(t) = stored.target_tokens {
            spec.chunking.target_tokens = t;
        }
        if let Some(o) = stored.overlap_tokens {
            spec.chunking.overlap_tokens = o;
        }
        if let Some(m) = stored.overlap_mode {
            spec.chunking.overlap_mode = m;
        }
        if let Some(h) = stored.contextual_headers {
            spec.contextual_headers = h;
        }
        spec
    }

    /// Apply explicit overrides (from `grans embed` flags) and validate.
    pub fn with_overrides(mut self, overrides: &EmbedOverrides) -> Result<Self> {
        if let Some(t) = overrides.target_tokens {
            self.chunking.target_tokens = t;
        }
        if let Some(o) = overrides.overlap_tokens {
            self.chunking.overlap_tokens = o;
        }
        if let Some(m) = overrides.overlap_mode {
            self.chunking.overlap_mode = m;
        }
        if let Some(h) = overrides.contextual_headers {
            self.contextual_headers = h;
        }
        self.validate()?;
        Ok(self)
    }

    fn validate(&self) -> Result<()> {
        let c = &self.chunking;
        if c.target_tokens == 0 {
            bail!("chunk target tokens must be greater than zero");
        }
        if c.target_tokens > c.max_tokens {
            bail!(
                "chunk target tokens ({}) cannot exceed the model limit ({})",
                c.target_tokens,
                c.max_tokens
            );
        }
        if c.overlap_tokens >= c.target_tokens {
            bail!(
                "chunk overlap tokens ({}) must be smaller than the target ({})",
                c.overlap_tokens,
                c.target_tokens
            );
        }
        Ok(())
    }

    /// The persisted dimensions of the spec, for change detection against
    /// what a database was embedded with.
    pub fn persisted_fields(&self) -> (usize, usize, OverlapMode, bool) {
        (
            self.chunking.target_tokens,
            self.chunking.overlap_tokens,
            self.chunking.overlap_mode,
            self.contextual_headers,
        )
    }

    /// Whether this spec differs from what the database's embeddings were
    /// built with. False when nothing is stored yet (a fresh embed is not
    /// a scheme change).
    pub fn differs_from_stored(&self, conn: &Connection) -> bool {
        let stored = store::get_chunking_metadata(conn);
        if stored.is_empty() {
            return false;
        }
        let defaults = Self::default_for(self.chunking.max_tokens);
        let stored_fields = (
            stored.target_tokens.unwrap_or(defaults.chunking.target_tokens),
            stored.overlap_tokens.unwrap_or(defaults.chunking.overlap_tokens),
            stored.overlap_mode.unwrap_or(defaults.chunking.overlap_mode),
            stored.contextual_headers.unwrap_or(defaults.contextual_headers),
        );
        self.persisted_fields() != stored_fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::create_tables(&conn).unwrap();
        conn
    }

    #[test]
    fn default_for_matches_chunking_defaults() {
        let spec = EmbedSpec::default_for(512);
        assert_eq!(spec.chunking.target_tokens, 348);
        assert_eq!(spec.chunking.overlap_tokens, 102);
        assert_eq!(spec.chunking.overlap_mode, OverlapMode::Chars);
        assert!(!spec.contextual_headers);
    }

    #[test]
    fn resolve_stored_defaults_when_nothing_stored() {
        let conn = test_db();
        let spec = EmbedSpec::resolve_stored(&conn, 512);
        assert_eq!(spec.persisted_fields(), EmbedSpec::default_for(512).persisted_fields());
    }

    #[test]
    fn resolve_stored_prefers_stored_params() {
        let conn = test_db();
        let variant = EmbedSpec {
            chunking: ChunkingConfig {
                target_tokens: 192,
                overlap_tokens: 48,
                overlap_mode: OverlapMode::Utterances,
                ..ChunkingConfig::from_max_length(512)
            },
            contextual_headers: true,
        };
        store::set_chunking_metadata(&conn, &variant).unwrap();

        let resolved = EmbedSpec::resolve_stored(&conn, 512);

        assert_eq!(resolved.chunking.target_tokens, 192);
        assert_eq!(resolved.chunking.overlap_tokens, 48);
        assert_eq!(resolved.chunking.overlap_mode, OverlapMode::Utterances);
        assert!(resolved.contextual_headers);
        // Model-derived limits still come from the binary.
        assert_eq!(resolved.chunking.max_tokens, 512);
    }

    #[test]
    fn overrides_win_and_validate() {
        let spec = EmbedSpec::default_for(512)
            .with_overrides(&EmbedOverrides {
                target_tokens: Some(192),
                overlap_tokens: Some(48),
                overlap_mode: Some(OverlapMode::Utterances),
                contextual_headers: Some(true),
            })
            .unwrap();

        assert_eq!(spec.persisted_fields(), (192, 48, OverlapMode::Utterances, true));
    }

    #[test]
    fn overrides_reject_overlap_not_below_target() {
        let err = EmbedSpec::default_for(512)
            .with_overrides(&EmbedOverrides {
                target_tokens: Some(100),
                overlap_tokens: Some(100),
                ..Default::default()
            })
            .unwrap_err();
        assert!(err.to_string().contains("overlap"));
    }

    #[test]
    fn overrides_reject_target_above_model_limit() {
        let err = EmbedSpec::default_for(512)
            .with_overrides(&EmbedOverrides {
                target_tokens: Some(1024),
                ..Default::default()
            })
            .unwrap_err();
        assert!(err.to_string().contains("model limit"));
    }

    #[test]
    fn differs_from_stored_false_on_fresh_db() {
        let conn = test_db();
        assert!(!EmbedSpec::default_for(512).differs_from_stored(&conn));
    }

    #[test]
    fn differs_from_stored_detects_change() {
        let conn = test_db();
        let stored = EmbedSpec::default_for(512);
        store::set_chunking_metadata(&conn, &stored).unwrap();

        assert!(!EmbedSpec::default_for(512).differs_from_stored(&conn));

        let variant = EmbedSpec {
            contextual_headers: true,
            ..EmbedSpec::default_for(512)
        };
        assert!(variant.differs_from_stored(&conn));
    }
}
