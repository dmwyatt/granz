//! Words: verbal tics, the user's signature vocabulary, and when terms
//! entered the conversation. These read transcript text, which the other
//! statistics never do, so they come from their own pass over it.
//!
//! Text is split into lowercase words of letters, digits and inner
//! apostrophes. Granola removes most single-word fillers ("um", "uh") from
//! transcripts, so the tics counted are the multi-word ones that survive.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use serde::Serialize;

use super::MeetingRow;
use super::mentions::{AbsentMention, NameMatcher, absent};
use super::people::names;

/// Phrases counted as verbal tics, in the user's own speech.
pub const TICS: &[&str] = &[
    "you know",
    "kind of",
    "sort of",
    "i mean",
    "basically",
    "actually",
    "literally",
];

/// Common words left out of vocabulary and term lists.
#[rustfmt::skip]
const STOPWORDS: &[&str] = &[
    "a", "about", "above", "after", "again", "against", "ah", "all", "also", "am", "an", "and",
    "any", "are", "aren't", "around", "as", "at", "back", "be", "because", "been", "before",
    "being", "below", "between", "both", "but", "by", "can", "can't", "come", "could", "couldn't",
    "did", "didn't", "do", "does", "doesn't", "doing", "don't", "down", "during", "each", "even",
    "few", "for", "from", "further", "get", "go", "going", "gonna", "good", "got", "had", "hadn't",
    "has", "hasn't", "have", "haven't", "having", "he", "he's", "her", "here", "hers", "herself",
    "hey", "him", "himself", "his", "how", "i", "i'd", "i'll", "i'm", "i've", "if", "in", "into",
    "is", "isn't", "it", "it's", "its", "itself", "just", "know", "let", "let's", "like", "make",
    "me", "mean", "might", "more", "most", "much", "my", "myself", "need", "no", "nor", "not",
    "now", "of", "off", "oh", "ok", "okay", "on", "once", "one", "only", "or", "other", "our",
    "ours", "ourselves", "out", "over", "own", "pretty", "really", "right", "said", "same", "say",
    "see", "she", "she's", "should", "shouldn't", "so", "some", "something", "such", "sure",
    "take", "than", "thank", "thanks", "that", "that's", "the", "their", "theirs", "them",
    "themselves", "then", "there", "there's", "these", "they", "they'll", "they're", "they've",
    "thing", "things", "think", "this", "those", "through", "to", "too", "um", "under", "until",
    "up", "uh", "us", "very", "want", "was", "wasn't", "way", "we", "we'll", "we're", "we've",
    "well", "were", "weren't", "what", "what's", "when", "where", "which", "while", "who", "whom",
    "why", "will", "with", "won't", "would", "wouldn't", "yeah", "yes", "you", "you'd", "you'll",
    "you're", "you've", "your", "yours", "yourself", "yourselves",
    // Fillers Granola sometimes keeps, and number words, which say nothing
    // about what a meeting was about.
    "hmm", "huh", "mhm", "mm", "mmm", "uh-huh", "two", "three", "four", "five", "six", "seven",
    "eight", "nine", "ten", "hundred", "thousand", "first", "second", "third",
];

/// The lowercase words of `text`.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '\u{2019}'))
        .map(|w| {
            w.trim_matches(|c| c == '\'' || c == '\u{2019}')
                .replace('\u{2019}', "'")
        })
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// Not a stopword, a number, or shorter than three letters.
pub fn is_content_word(word: &str) -> bool {
    word.len() > 2 && !STOPWORDS.contains(&word) && !word.chars().all(|c| c.is_ascii_digit())
}

/// Occurrences of the word sequence `phrase` in `words`.
fn count_phrase(words: &[String], phrase: &[String]) -> u32 {
    if phrase.is_empty() || phrase.len() > words.len() {
        return 0;
    }
    words.windows(phrase.len()).filter(|w| *w == phrase).count() as u32
}

/// The user's tics in one meeting.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct MeetingTics {
    pub my_words: u64,
    /// Per entry of [`TICS`], in order.
    pub counts: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TicMonth {
    pub month: NaiveDate,
    pub my_words: u64,
    /// Occurrences per thousand of the user's words, per entry of [`TICS`].
    pub per_thousand: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignatureWord {
    pub word: String,
    pub mine: u64,
    pub others: u64,
    /// How much more the user says it, as a z-score of the log-odds ratio
    /// with an informative Dirichlet prior (Monroe, Colaresi and Quinn,
    /// 2008). Above about 2 is unlikely to be chance.
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TermLife {
    pub term: String,
    pub mentions: u64,
    pub first_month: Option<NaiveDate>,
    pub peak_month: Option<NaiveDate>,
    /// Mentions per month, every month of the range included.
    pub monthly: Vec<(NaiveDate, u64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WordsReport {
    pub tics: Vec<&'static str>,
    pub tics_by_month: Vec<TicMonth>,
    /// Most distinctive first.
    pub signature: Vec<SignatureWord>,
    /// Words first heard after the first tenth of the range's words, most
    /// mentioned first.
    pub emerging: Vec<TermLife>,
    /// The terms asked for, in the order given.
    pub terms: Vec<TermLife>,
    /// People whose full name was said in meetings they were not invited
    /// to, most such meetings first.
    pub mentioned_while_absent: Vec<AbsentMention>,
}

/// Accumulates one pass over the transcript text.
pub struct WordCounter {
    tz: FixedOffset,
    tic_phrases: Vec<Vec<String>>,
    term_phrases: Vec<(String, Vec<String>)>,
    /// Per meeting id.
    meeting_tics: HashMap<String, MeetingTics>,
    tic_months: BTreeMap<NaiveDate, MeetingTics>,
    mine: HashMap<String, u64>,
    others: HashMap<String, u64>,
    word_months: HashMap<String, BTreeMap<NaiveDate, u64>>,
    term_months: Vec<BTreeMap<NaiveDate, u64>>,
    /// Every word heard, per month.
    month_words: BTreeMap<NaiveDate, u64>,
    names: NameMatcher,
    /// Full-name mentions per meeting id, then per email.
    mentions: HashMap<String, BTreeMap<String, u32>>,
}

const SIGNATURE_MIN_MINE: u64 = 20;
const SIGNATURE_LISTED: usize = 30;
const EMERGING_MIN_MENTIONS: u64 = 30;
const EMERGING_LISTED: usize = 20;
const PRIOR_STRENGTH: f64 = 500.0;

impl WordCounter {
    /// `people` are the `(email, full name)` pairs to listen for.
    pub fn new(tz: FixedOffset, terms: &[String], people: &[(String, String)]) -> Self {
        WordCounter {
            tz,
            tic_phrases: TICS.iter().map(|t| words(t)).collect(),
            term_phrases: terms.iter().map(|t| (t.clone(), words(t))).collect(),
            meeting_tics: HashMap::new(),
            tic_months: BTreeMap::new(),
            mine: HashMap::new(),
            others: HashMap::new(),
            word_months: HashMap::new(),
            term_months: vec![BTreeMap::new(); terms.len()],
            month_words: BTreeMap::new(),
            names: NameMatcher::new(people),
            mentions: HashMap::new(),
        }
    }

    /// Count one utterance of the meeting `meeting_id`, created at
    /// `created_at`. `mine` is true for the user's microphone.
    pub fn add(&mut self, meeting_id: &str, created_at: DateTime<Utc>, mine: bool, text: &str) {
        let month = created_at
            .with_timezone(&self.tz)
            .date_naive()
            .with_day(1)
            .expect("every month has a first day");
        let tokens = words(text);
        *self.month_words.entry(month).or_default() += tokens.len() as u64;
        for email in self.names.find(&tokens) {
            *self
                .mentions
                .entry(meeting_id.to_string())
                .or_default()
                .entry(email.to_string())
                .or_default() += 1;
        }

        for ((_, phrase), months) in self.term_phrases.iter().zip(&mut self.term_months) {
            let n = count_phrase(&tokens, phrase);
            if n > 0 {
                *months.entry(month).or_default() += u64::from(n);
            }
        }
        for word in tokens.iter().filter(|w| is_content_word(w)) {
            *self
                .word_months
                .entry(word.clone())
                .or_default()
                .entry(month)
                .or_default() += 1;
            let side = if mine {
                &mut self.mine
            } else {
                &mut self.others
            };
            *side.entry(word.clone()).or_default() += 1;
        }
        if mine {
            let counts: Vec<u32> = self
                .tic_phrases
                .iter()
                .map(|p| count_phrase(&tokens, p))
                .collect();
            for tics in [
                self.meeting_tics.entry(meeting_id.to_string()).or_default(),
                self.tic_months.entry(month).or_default(),
            ] {
                tics.my_words += tokens.len() as u64;
                tics.counts.resize(TICS.len(), 0);
                for (total, n) in tics.counts.iter_mut().zip(&counts) {
                    *total += n;
                }
            }
        }
    }

    /// Each meeting's tics, for callers that slice them further.
    pub fn meeting_tics(&self) -> &HashMap<String, MeetingTics> {
        &self.meeting_tics
    }

    /// Full-name mentions per meeting id, then per email.
    pub fn mentions(&self) -> &HashMap<String, BTreeMap<String, u32>> {
        &self.mentions
    }

    /// `rows` are the meetings whose text was counted, for who was invited.
    pub fn report(&self, rows: &[MeetingRow]) -> WordsReport {
        let months: Vec<NaiveDate> = self.month_words.keys().copied().collect();
        WordsReport {
            tics: TICS.to_vec(),
            tics_by_month: self
                .tic_months
                .iter()
                .map(|(month, t)| TicMonth {
                    month: *month,
                    my_words: t.my_words,
                    per_thousand: t
                        .counts
                        .iter()
                        .map(|n| f64::from(*n) * 1000.0 / t.my_words.max(1) as f64)
                        .collect(),
                })
                .collect(),
            signature: self.signature(),
            emerging: self.emerging(&months),
            terms: self
                .term_phrases
                .iter()
                .zip(&self.term_months)
                .map(|((term, _), counts)| life(term, counts, &months))
                .collect(),
            mentioned_while_absent: absent(rows, &self.mentions, &names(rows)),
        }
    }

    fn signature(&self) -> Vec<SignatureWord> {
        let n_mine: u64 = self.mine.values().sum();
        let n_others: u64 = self.others.values().sum();
        let n_all = (n_mine + n_others).max(1) as f64;
        let mut words: Vec<SignatureWord> = self
            .mine
            .iter()
            .filter(|(_, n)| **n >= SIGNATURE_MIN_MINE)
            .map(|(word, &mine)| {
                let others = self.others.get(word).copied().unwrap_or(0);
                let prior = PRIOR_STRENGTH * (mine + others) as f64 / n_all;
                let odds = |y: u64, n: u64| {
                    let y = y as f64 + prior;
                    (y / (n as f64 + PRIOR_STRENGTH - y)).ln()
                };
                let delta = odds(mine, n_mine) - odds(others, n_others);
                let variance = 1.0 / (mine as f64 + prior) + 1.0 / (others as f64 + prior);
                SignatureWord {
                    word: word.clone(),
                    mine,
                    others,
                    score: delta / variance.sqrt(),
                }
            })
            .collect();
        words.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.word.cmp(&b.word))
        });
        words.truncate(SIGNATURE_LISTED);
        words
    }

    /// Words whose first month comes after the first tenth of all the words
    /// heard. Measuring the start in words rather than months keeps a sparse
    /// first few months from making ordinary words look new.
    fn emerging(&self, months: &[NaiveDate]) -> Vec<TermLife> {
        let total: u64 = self.month_words.values().sum();
        let mut heard = 0;
        let Some(settled) = self.month_words.iter().find_map(|(month, n)| {
            heard += n;
            (heard * 10 >= total).then_some(*month)
        }) else {
            return vec![];
        };
        let mut terms: Vec<TermLife> = self
            .word_months
            .iter()
            .filter(|(_, counts)| counts.keys().next().is_some_and(|first| *first > settled))
            .filter(|(_, counts)| counts.values().sum::<u64>() >= EMERGING_MIN_MENTIONS)
            .map(|(word, counts)| life(word, counts, months))
            .collect();
        terms.sort_by(|a, b| {
            b.mentions
                .cmp(&a.mentions)
                .then_with(|| a.term.cmp(&b.term))
        });
        terms.truncate(EMERGING_LISTED);
        terms
    }
}

fn life(term: &str, counts: &BTreeMap<NaiveDate, u64>, months: &[NaiveDate]) -> TermLife {
    TermLife {
        term: term.to_string(),
        mentions: counts.values().sum(),
        first_month: counts.keys().next().copied(),
        peak_month: counts
            .iter()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            .map(|(m, _)| *m),
        monthly: months
            .iter()
            .map(|m| (*m, counts.get(m).copied().unwrap_or(0)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn month(y: i32, m: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, 1).unwrap()
    }

    #[test]
    fn words_are_lowercase_and_keep_inner_apostrophes() {
        assert_eq!(
            words("We're 'done', OK? Don\u{2019}t."),
            vec!["we're", "done", "ok", "don't"]
        );
    }

    #[test]
    fn tics_are_counted_in_my_speech_only() {
        let mut c = WordCounter::new(utc(), &[], &[]);
        c.add(
            "m1",
            at("2026-03-02T10:00:00Z"),
            true,
            "You know, it is, you know, fine.",
        );
        c.add("m1", at("2026-03-02T10:01:00Z"), false, "You know what?");

        let t = &c.meeting_tics()["m1"];
        assert_eq!((t.my_words, t.counts[0]), (7, 2));
    }

    #[test]
    fn tic_rate_is_per_thousand_of_my_words() {
        let mut c = WordCounter::new(utc(), &[], &[]);
        let text = format!("basically {}", "word ".repeat(499));
        c.add("m1", at("2026-03-02T10:00:00Z"), true, &text);

        let basically = TICS.iter().position(|t| *t == "basically").unwrap();
        assert_eq!(c.report(&[]).tics_by_month[0].per_thousand[basically], 2.0);
    }

    #[test]
    fn a_term_can_be_a_phrase() {
        let mut c = WordCounter::new(utc(), &["launch plan".to_string()], &[]);
        c.add(
            "m1",
            at("2026-03-02T10:00:00Z"),
            false,
            "The launch plan, and the launch.",
        );
        c.add(
            "m2",
            at("2026-04-02T10:00:00Z"),
            true,
            "Launch plan again; launch plan.",
        );

        let term = &c.report(&[]).terms[0];
        assert_eq!(
            (term.mentions, term.first_month, term.peak_month),
            (3, Some(month(2026, 3)), Some(month(2026, 4)))
        );
    }

    #[test]
    fn a_word_i_say_and_others_do_not_is_my_signature() {
        let mut c = WordCounter::new(utc(), &[], &[]);
        for _ in 0..30 {
            c.add("m1", at("2026-03-02T10:00:00Z"), true, "frankly the budget");
            c.add(
                "m1",
                at("2026-03-02T10:00:00Z"),
                false,
                "the budget is the budget",
            );
        }

        let signature = c.report(&[]).signature;
        assert_eq!(signature[0].word, "frankly");
    }

    #[test]
    fn emerging_terms_first_appear_after_the_first_tenth_of_the_words() {
        let mut c = WordCounter::new(utc(), &[], &[]);
        c.add(
            "m0",
            at("2026-01-05T10:00:00Z"),
            false,
            &"roadmap ".repeat(100),
        );
        c.add(
            "m1",
            at("2026-02-05T10:00:00Z"),
            false,
            &"zephyr ".repeat(40),
        );
        c.add(
            "m2",
            at("2026-02-06T10:00:00Z"),
            false,
            &"roadmap ".repeat(60),
        );

        let emerging: Vec<_> = c.report(&[]).emerging.into_iter().map(|t| t.term).collect();
        assert_eq!(emerging, vec!["zephyr"]);
    }

    #[test]
    fn stopwords_are_not_vocabulary() {
        let mut c = WordCounter::new(utc(), &[], &[]);
        for _ in 0..30 {
            c.add("m1", at("2026-03-02T10:00:00Z"), true, "because");
        }

        assert!(c.report(&[]).signature.is_empty());
    }
}
