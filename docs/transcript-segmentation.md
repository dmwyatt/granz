# Transcript Utterance Segmentation

How Granola splits speech into `transcript_utterances` rows, established empirically in July 2026 by analyzing a local database (872 documents, 857 with transcripts, 487,112 utterances). Duration, overlap, and boundary findings come from the 15 most recent meetings at the time (7,272 utterances with populated `source`); length and gap distributions come from the full corpus. None of this is documented by Granola; treat it as observed behavior that could change.

## Channel model

- Each utterance carries `source`: `microphone` (the local user) or `system` (everyone else, plus any audio the machine plays). Rows synced before the column existed have NULL `source`.
- The two channels are captured and segmented independently. They overlap in time: roughly 9% of microphone utterances overlap a system utterance. Interleaving the channels by `start_timestamp` approximates turn order but is not exact.
- Utterances on the same channel never overlap each other.

## Boundary model

Segmentation is silence-based endpointing with text post-processing:

- 100% of utterance texts end with terminal punctuation (`.`, `?`, `!`, `…`), and none start with a lowercase letter. The pipeline emits complete, punctuated sentences, which implies the raw ASR output is cleaned before storage. Whatever does that cleaning could also merge or rewrite segments in ways not visible from the stored data.
- Consecutive same-channel utterances are separated by real silence: the median gap is 1.8s, and near-zero gaps (under 0.25s) occur in under 1% of cases. Boundaries happen when the speaker pauses, not on a fixed clock.
- `is_final` is 1 on every stored row; streaming partials are not persisted.
- There is no maximum segment length. An unbroken voice keeps extending one utterance: about 20% of utterances contain multiple sentences, and the longest observed is 11,963 characters (198 sentences, about 6 minutes). Inspected long utterances were a single voice throughout, consistent with a monologue or played media rather than multi-party speech, which follows from the mechanism: only an uninterrupted voice avoids a silence boundary for that long.

## Length and duration

Full corpus, characters per utterance: p50 = 32, p75 = 64, p90 = 105, p99 = 315, max = 11,963. 7.8% exceed 120 characters.

Recent meetings, seconds per utterance: microphone p50 = 1.1, p90 = 5.4, max = 21; system p50 = 2.2, p90 = 10.2, max = 94. Speech rate centers around 18 characters/second.

## No speaker segmentation

Granola does no diarization, and the system channel is not split on speaker change. When one remote speaker hands off to another without a pause, both land in the same utterance. This is directly observable: scanning system utterances for a question immediately followed by a short answer ("...? Yeah.") finds 816 matches (0.27% of system utterances), and inspection confirms many are genuine two-speaker exchanges fused into one row. That pattern only catches question-answer handoffs, so the true fused rate is higher. Fused utterances are rare but real; any speaker-attribution layer needs to allow more than one speaker per utterance.

Because texts are clean sentences, sub-utterance attribution (if ever needed) can address sentence indices within an utterance rather than character offsets.

## Run structure

A "run" is a maximal sequence of consecutive system utterances (speech between the local user's own utterances). About 70% of runs are 1 to 2 utterances, but they hold only 14 to 22% of system utterances; most system speech sits in long runs. In meetings with 3+ attendees, run length is p50 = 1, p90 = 10, p99 = 85, max = 2,108 utterances. Long runs in multi-party meetings almost certainly span several speakers, so "one run, one speaker" is not a safe assumption there.

Within runs, the gap between consecutive system utterances is p50 = 0.6s; 18.6% of junctions exceed 2s and 2.6% exceed 5s. Since same-channel gaps are literal detected silences, gaps over a couple of seconds are usable as candidate turn boundaries.

## Reproducing

The numbers come from read-only SQL over `grans.db` (`transcript_utterances` joined with `document_people` for meeting-size bucketing): length and duration percentiles, same-channel and cross-channel timestamp comparisons, run grouping by consecutive `source`, and regex checks on text boundaries. No fixtures are checked in because the analysis runs against private meeting data; rerun the queries locally to refresh the findings.
