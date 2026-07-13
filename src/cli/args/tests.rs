use super::*;
use clap::CommandFactory;

#[test]
fn verify_cli() {
    Cli::command().debug_assert();
}

#[test]
fn search_semantic_flag_is_rejected() {
    // The standalone semantic retrieval mode was removed (#59); the flag
    // must fail parsing rather than silently doing something else.
    let result = Cli::try_parse_from(["grans", "search", "q", "--semantic"]);
    assert!(result.is_err(), "--semantic should be an unknown flag");
}

#[test]
fn search_keyword_flag_is_rejected() {
    // Plain lexical lookup is the grep verb now (#65); the flag must
    // fail parsing rather than silently doing something else.
    let result = Cli::try_parse_from(["grans", "search", "q", "--keyword"]);
    assert!(result.is_err(), "--keyword should be an unknown flag");
}

#[test]
fn search_speaker_flag_is_rejected() {
    // Speaker attribution needs exact transcript matching, which only
    // grep's complete lookup honors (#65).
    let result = Cli::try_parse_from(["grans", "search", "q", "--speaker", "me"]);
    assert!(result.is_err(), "--speaker should be an unknown flag");
}

#[test]
fn search_hybrid_flag_is_rejected() {
    // Hybrid retrieval is search's only behavior; the vestigial flag is
    // gone (#65).
    let result = Cli::try_parse_from(["grans", "search", "q", "--hybrid"]);
    assert!(result.is_err(), "--hybrid should be an unknown flag");
}

#[test]
fn search_fast_parses() {
    let cli = Cli::try_parse_from(["grans", "search", "q", "--fast"]).unwrap();
    let Commands::Search { fast, .. } = &cli.command else {
        panic!("expected search subcommand");
    };
    assert!(*fast);
}

#[test]
fn search_context_composes_with_other_flags() {
    // --context expands cards; it conflicts with nothing.
    for extra in [
        &["--fast"][..],
        &["--min-score", "0.4"][..],
        &["--matches", "3"][..],
        &[][..],
    ] {
        let mut argv = vec!["grans", "search", "q", "--context", "2"];
        argv.extend_from_slice(extra);
        let result = Cli::try_parse_from(argv);
        assert!(result.is_ok(), "--context {extra:?} should parse");
    }
}

#[test]
fn search_min_score_parses() {
    let cli =
        Cli::try_parse_from(["grans", "search", "q", "--min-score", "0.4"]).unwrap();
    let Commands::Search { min_score, .. } = &cli.command else {
        panic!("expected search subcommand");
    };
    assert_eq!(*min_score, Some(0.4));
}

#[test]
fn search_min_score_conflicts_with_fast() {
    // Only the rerank stage produces the relevance score --min-score
    // thresholds, and --fast skips that stage.
    let result =
        Cli::try_parse_from(["grans", "search", "q", "--min-score", "0.4", "--fast"]);
    assert!(result.is_err(), "--min-score --fast should conflict");
}

#[test]
fn grep_parses_with_lookup_flags() {
    let cli = Cli::try_parse_from([
        "grans", "grep", "kumquat", "--speaker", "me", "--in", "titles,transcripts",
        "--meeting", "standup", "--context", "2", "--matches", "3", "--limit", "5",
        "--from", "2026-01-01", "--to", "2026-02-01", "--include-deleted",
    ])
    .unwrap();
    let Commands::Grep {
        query,
        speaker,
        r#in,
        meeting,
        context,
        matches,
        limit,
        include_deleted,
        ..
    } = &cli.command
    else {
        panic!("expected grep subcommand");
    };
    assert_eq!(query, "kumquat");
    assert_eq!(*speaker, Some(SpeakerFilter::Me));
    assert_eq!(r#in, &vec![SearchTarget::Titles, SearchTarget::Transcripts]);
    assert_eq!(meeting.as_deref(), Some("standup"));
    assert_eq!(*context, 2);
    assert_eq!(*matches, 3);
    assert_eq!(*limit, 5);
    assert!(*include_deleted);
}

#[test]
fn grep_visible_alias_g_parses() {
    let cli = Cli::try_parse_from(["grans", "g", "kumquat"]).unwrap();
    assert!(matches!(cli.command, Commands::Grep { .. }));
}

fn grep_in_targets(cli: &Cli) -> &[SearchTarget] {
    match &cli.command {
        Commands::Grep { r#in, .. } => r#in,
        _ => panic!("expected grep subcommand"),
    }
}

#[test]
fn grep_in_defaults_to_every_target() {
    // Omitting --in searches every source (#74 preserves this default).
    let cli = Cli::try_parse_from(["grans", "grep", "budget"]).unwrap();
    assert_eq!(grep_in_targets(&cli), SearchTarget::all().as_slice());
}

#[test]
fn grep_in_parses_a_valid_comma_separated_list() {
    let cli =
        Cli::try_parse_from(["grans", "grep", "budget", "--in", "titles,transcripts"]).unwrap();
    assert_eq!(
        grep_in_targets(&cli),
        &[SearchTarget::Titles, SearchTarget::Transcripts]
    );
}

#[test]
fn grep_in_rejects_an_unknown_target() {
    // `transcript` (singular typo) must fail loudly instead of collapsing to
    // an empty target set that greps nothing (#74). The error names the bad
    // value and lists the valid ones.
    let err = Cli::try_parse_from(["grans", "grep", "budget", "--in", "transcript"])
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("transcript"), "error should name the bad value: {msg}");
    assert!(
        msg.contains("titles") && msg.contains("transcripts") && msg.contains("notes")
            && msg.contains("panels"),
        "error should list the valid targets: {msg}"
    );
}

#[test]
fn grep_in_rejects_an_unknown_target_inside_a_valid_list() {
    // `titles,transcript` must not silently search titles only (#74); one
    // bad token fails the whole list.
    let result = Cli::try_parse_from(["grans", "grep", "budget", "--in", "titles,transcript"]);
    assert!(result.is_err(), "an unknown token anywhere must fail the list");
}

#[test]
fn search_in_rejects_an_unknown_target() {
    // The rejection applies to search's --in as well.
    let result = Cli::try_parse_from(["grans", "search", "budget", "--in", "transcript"]);
    assert!(result.is_err(), "search --in must reject unknown targets");
}

#[test]
fn grep_rejects_ranked_search_flags() {
    // Ranked-pipeline flags belong to search; grep never runs models,
    // so none of them parse here.
    for extra in [
        &["--fast"][..],
        &["--min-score", "0.4"][..],
        &["--yes"][..],
        &["--keyword"][..],
        &["--hybrid"][..],
    ] {
        let mut argv = vec!["grans", "grep", "q"];
        argv.extend_from_slice(extra);
        let result = Cli::try_parse_from(argv);
        assert!(result.is_err(), "grep {extra:?} should be rejected");
    }
}

fn quality_compare(cli: &Cli) -> &[QualityMode] {
    match &cli.command {
        Commands::Benchmark {
            action: BenchmarkAction::Quality { compare, .. },
        } => compare,
        _ => panic!("expected benchmark quality subcommand"),
    }
}

fn embed_experiment_flags(cli: &Cli) -> (Option<usize>, Option<usize>, Option<String>, Option<bool>) {
    match &cli.command {
        Commands::Embed {
            chunk_target_tokens,
            chunk_overlap_tokens,
            overlap_mode,
            contextual_headers,
            ..
        } => (
            *chunk_target_tokens,
            *chunk_overlap_tokens,
            overlap_mode.clone(),
            *contextual_headers,
        ),
        _ => panic!("expected embed subcommand"),
    }
}

#[test]
fn embed_experiment_flags_default_to_none() {
    let cli = Cli::try_parse_from(["grans", "embed"]).unwrap();
    assert_eq!(embed_experiment_flags(&cli), (None, None, None, None));
}

#[test]
fn embed_experiment_flags_parse() {
    let cli = Cli::try_parse_from([
        "grans",
        "embed",
        "--chunk-target-tokens",
        "192",
        "--chunk-overlap-tokens",
        "48",
        "--overlap-mode",
        "utterances",
        "--contextual-headers",
    ])
    .unwrap();
    assert_eq!(
        embed_experiment_flags(&cli),
        (Some(192), Some(48), Some("utterances".to_string()), Some(true))
    );
}

#[test]
fn embed_contextual_headers_can_be_forced_off() {
    let cli =
        Cli::try_parse_from(["grans", "embed", "--contextual-headers=false"]).unwrap();
    assert_eq!(embed_experiment_flags(&cli).3, Some(false));
}

#[test]
fn embed_overlap_mode_rejects_unknown_value() {
    let result = Cli::try_parse_from(["grans", "embed", "--overlap-mode", "bogus"]);
    assert!(result.is_err());
}

#[test]
fn benchmark_quality_title_boost_weight_parses_and_defaults_to_none() {
    let cli = Cli::try_parse_from([
        "grans", "benchmark", "quality", "--file", "golden.json",
    ])
    .unwrap();
    let Commands::Benchmark { action: BenchmarkAction::Quality { title_boost_weight, .. } } =
        &cli.command
    else {
        panic!("expected benchmark quality subcommand");
    };
    assert_eq!(*title_boost_weight, None);

    let cli = Cli::try_parse_from([
        "grans", "benchmark", "quality", "--file", "golden.json",
        "--title-boost-weight", "0.3",
    ])
    .unwrap();
    let Commands::Benchmark { action: BenchmarkAction::Quality { title_boost_weight, .. } } =
        &cli.command
    else {
        panic!("expected benchmark quality subcommand");
    };
    assert_eq!(*title_boost_weight, Some(0.3));
}

#[test]
fn benchmark_quality_compare_accepts_comma_separated_modes() {
    let cli = Cli::try_parse_from([
        "grans",
        "benchmark",
        "quality",
        "--file",
        "golden.json",
        "--compare",
        "fts,semantic",
    ])
    .unwrap();
    assert_eq!(
        quality_compare(&cli),
        &[QualityMode::Fts, QualityMode::Semantic]
    );
}

#[test]
fn benchmark_quality_mode_conflicts_with_compare() {
    let result = Cli::try_parse_from([
        "grans",
        "benchmark",
        "quality",
        "--file",
        "golden.json",
        "--mode",
        "fts",
        "--compare",
        "fts,semantic",
    ]);
    assert!(result.is_err());
}

#[test]
fn benchmark_quality_dump_candidates_conflicts_with_compare() {
    let result = Cli::try_parse_from([
        "grans",
        "benchmark",
        "quality",
        "--file",
        "golden.json",
        "--compare",
        "rerank-jina,rerank-bge",
        "--dump-candidates",
        "dump.jsonl",
    ]);
    assert!(result.is_err());
}

#[test]
fn benchmark_quality_note_requires_record() {
    let result = Cli::try_parse_from([
        "grans",
        "benchmark",
        "quality",
        "--file",
        "golden.json",
        "--note",
        "some note",
    ]);
    assert!(result.is_err());
}

fn transcripts_action(cli: &Cli) -> &SyncAction {
    match &cli.command {
        Commands::Sync { action: Some(action), .. } => action,
        _ => panic!("expected sync subcommand"),
    }
}

#[test]
fn sync_transcripts_accepts_positional_document_id() {
    let cli = Cli::try_parse_from(["grans", "sync", "transcripts", "doc-1"]).unwrap();
    match transcripts_action(&cli) {
        SyncAction::Transcripts { document_id, .. } => {
            assert_eq!(document_id.as_deref(), Some("doc-1"));
        }
        _ => panic!("expected transcripts action"),
    }
}

#[test]
fn sync_transcripts_positional_conflicts_with_limit() {
    let result = Cli::try_parse_from(["grans", "sync", "transcripts", "doc-1", "--limit", "5"]);
    assert!(result.is_err());
}

#[test]
fn sync_transcripts_positional_allows_embed() {
    let cli =
        Cli::try_parse_from(["grans", "sync", "transcripts", "doc-1", "--embed"]).unwrap();
    match transcripts_action(&cli) {
        SyncAction::Transcripts { document_id, embed, .. } => {
            assert_eq!(document_id.as_deref(), Some("doc-1"));
            assert!(*embed);
        }
        _ => panic!("expected transcripts action"),
    }
}

#[test]
fn sync_transcripts_positional_allows_dry_run() {
    let cli =
        Cli::try_parse_from(["grans", "sync", "transcripts", "doc-1", "--dry-run"]).unwrap();
    match &cli.command {
        Commands::Sync { dry_run, .. } => assert!(*dry_run),
        _ => panic!("expected sync subcommand"),
    }
}
