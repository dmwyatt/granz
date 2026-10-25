//! Topics, from the embeddings `grans embed` stores: which meetings are
//! about the same things, which recurring meetings keep repeating
//! themselves, which series overlap, and which meetings are unlike any
//! other.
//!
//! A meeting is represented by the mean of its transcript windows'
//! vectors, normalized, so similarity is the cosine between meetings.
//! Meetings `grans embed` has not reached are left out.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;

use super::words::{is_content_word, words};
use super::{MeetingRow, median};

/// How many topic groups meetings are sorted into.
pub const CLUSTERS: usize = 8;
const LISTED: usize = 10;
/// Series need this many embedded meetings to be compared as a whole.
const TWIN_MIN_MEETINGS: usize = 3;
const KMEANS_ROUNDS: usize = 30;
const POWER_ROUNDS: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeetingTopic {
    pub id: String,
    /// Position on the map: the first two principal components.
    pub x: f32,
    pub y: f32,
    pub cluster: usize,
    /// Similarity to the most similar other meeting.
    pub nearest: f32,
    /// Similarity to the previous meeting of its series.
    pub previous_in_series: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cluster {
    pub id: usize,
    /// Words its meetings' titles use far more than other titles do.
    pub label: Vec<String>,
    pub meetings: usize,
    pub first: DateTime<Utc>,
    pub last: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rehash {
    pub series: String,
    pub title: String,
    /// Consecutive pairs compared.
    pub pairs: usize,
    pub typical_similarity: f32,
    pub latest_similarity: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Twin {
    pub a: String,
    pub a_title: String,
    pub b: String,
    pub b_title: String,
    pub similarity: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outlier {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub nearest: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TopicsReport {
    pub meetings: usize,
    pub embedded: usize,
    pub clusters: Vec<Cluster>,
    /// Series whose consecutive meetings are most alike first.
    pub rehash: Vec<Rehash>,
    /// Most alike first.
    pub twins: Vec<Twin>,
    /// Least like any other meeting first.
    pub outliers: Vec<Outlier>,
    pub map: Vec<MeetingTopic>,
}

fn normalized(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        v.to_vec()
    } else {
        v.iter().map(|x| x / norm).collect()
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// A similarity to three places, as an integer that orders like it.
fn milli(similarity: f32) -> i64 {
    (similarity * 1000.0).round() as i64
}

pub fn report(rows: &[MeetingRow], vectors: &HashMap<String, Vec<f32>>) -> TopicsReport {
    let mut embedded: Vec<(&MeetingRow, Vec<f32>)> = rows
        .iter()
        .filter_map(|r| Some((r, normalized(vectors.get(&r.id)?))))
        .collect();
    embedded.sort_by_key(|(r, _)| r.created_at);
    let unit: Vec<&[f32]> = embedded.iter().map(|(_, v)| v.as_slice()).collect();

    let nearest = nearest(&unit);
    let assignment = clusters(&unit);
    let (xs, ys) = projection(&unit);
    let previous = previous_in_series(&embedded);

    TopicsReport {
        meetings: rows.len(),
        embedded: embedded.len(),
        clusters: describe_clusters(&embedded, &assignment),
        rehash: rehash(&embedded, &previous),
        twins: twins(&embedded),
        outliers: outliers(&embedded, &nearest),
        map: embedded
            .iter()
            .enumerate()
            .map(|(i, (row, _))| MeetingTopic {
                id: row.id.clone(),
                x: xs[i],
                y: ys[i],
                cluster: assignment[i],
                nearest: nearest[i],
                previous_in_series: previous[i],
            })
            .collect(),
    }
}

fn nearest(unit: &[&[f32]]) -> Vec<f32> {
    let mut best = vec![f32::NEG_INFINITY; unit.len()];
    for i in 0..unit.len() {
        for j in i + 1..unit.len() {
            let s = dot(unit[i], unit[j]);
            best[i] = best[i].max(s);
            best[j] = best[j].max(s);
        }
    }
    best
}

/// Each meeting's similarity to the previous embedded meeting of its
/// series. `embedded` is in creation order.
fn previous_in_series(embedded: &[(&MeetingRow, Vec<f32>)]) -> Vec<Option<f32>> {
    let mut last: HashMap<&str, usize> = HashMap::new();
    embedded
        .iter()
        .enumerate()
        .map(|(i, (row, v))| {
            let series = row.series.as_deref()?;
            let previous = last.insert(series, i);
            previous.map(|p| dot(&embedded[p].1, v))
        })
        .collect()
}

fn rehash(embedded: &[(&MeetingRow, Vec<f32>)], previous: &[Option<f32>]) -> Vec<Rehash> {
    let mut by_series: BTreeMap<&str, Vec<(usize, f32)>> = BTreeMap::new();
    for (i, similarity) in previous.iter().enumerate() {
        if let (Some(s), Some(series)) = (similarity, embedded[i].0.series.as_deref()) {
            by_series.entry(series).or_default().push((i, *s));
        }
    }
    let mut out: Vec<Rehash> = by_series
        .into_iter()
        .filter_map(|(series, pairs)| {
            let (latest, latest_similarity) = *pairs.last()?;
            let typical = median(pairs.iter().map(|(_, s)| milli(*s)))? as f32 / 1000.0;
            Some(Rehash {
                series: series.to_string(),
                title: embedded[latest].0.title.clone(),
                pairs: pairs.len(),
                typical_similarity: typical,
                latest_similarity,
            })
        })
        .filter(|r| r.pairs >= 2)
        .collect();
    out.sort_by(|a, b| {
        b.typical_similarity
            .total_cmp(&a.typical_similarity)
            .then_with(|| a.title.cmp(&b.title))
    });
    out
}

fn twins(embedded: &[(&MeetingRow, Vec<f32>)]) -> Vec<Twin> {
    let mut members: BTreeMap<&str, Vec<&(&MeetingRow, Vec<f32>)>> = BTreeMap::new();
    for m in embedded {
        if let Some(series) = m.0.series.as_deref() {
            members.entry(series).or_default().push(m);
        }
    }
    let centroids: Vec<(&str, &str, Vec<f32>)> = members
        .into_iter()
        .filter(|(_, ms)| ms.len() >= TWIN_MIN_MEETINGS)
        .map(|(series, ms)| {
            let mut sum = vec![0.0; ms[0].1.len()];
            for (_, v) in &ms {
                for (s, x) in sum.iter_mut().zip(v) {
                    *s += x;
                }
            }
            (series, ms[ms.len() - 1].0.title.as_str(), normalized(&sum))
        })
        .collect();
    let mut pairs: Vec<Twin> = Vec::new();
    for (i, a) in centroids.iter().enumerate() {
        // Same-titled series are one meeting split by a calendar edit, not
        // two meetings covering the same ground.
        for b in centroids[i + 1..]
            .iter()
            .filter(|b| !b.1.eq_ignore_ascii_case(a.1))
        {
            pairs.push(Twin {
                a: a.0.to_string(),
                a_title: a.1.to_string(),
                b: b.0.to_string(),
                b_title: b.1.to_string(),
                similarity: dot(&a.2, &b.2),
            });
        }
    }
    pairs.sort_by(|x, y| y.similarity.total_cmp(&x.similarity));
    pairs.truncate(LISTED);
    pairs
}

fn outliers(embedded: &[(&MeetingRow, Vec<f32>)], nearest: &[f32]) -> Vec<Outlier> {
    let mut out: Vec<Outlier> = embedded
        .iter()
        .zip(nearest)
        .filter(|(_, n)| n.is_finite())
        .map(|((row, _), n)| Outlier {
            id: row.id.clone(),
            title: row.title.clone(),
            created_at: row.created_at,
            nearest: *n,
        })
        .collect();
    out.sort_by(|a, b| a.nearest.total_cmp(&b.nearest));
    out.truncate(LISTED);
    out
}

/// Spherical k-means: k-means++ seeding from a fixed seed, so the same
/// meetings always give the same groups, then rounds of assigning each
/// meeting to its most similar centre.
fn clusters(unit: &[&[f32]]) -> Vec<usize> {
    if unit.is_empty() {
        return vec![];
    }
    let k = CLUSTERS.min(unit.len());
    let mut rng = StdRng::seed_from_u64(172);
    let mut centres: Vec<Vec<f32>> = vec![unit[rng.gen_range(0..unit.len())].to_vec()];
    while centres.len() < k {
        let distance: Vec<f32> = unit
            .iter()
            .map(|v| {
                let best = centres.iter().map(|c| dot(c, v)).fold(f32::MIN, f32::max);
                (1.0 - best).max(0.0).powi(2)
            })
            .collect();
        let total: f32 = distance.iter().sum();
        if total == 0.0 {
            break;
        }
        let mut pick = rng.gen_range(0.0..total);
        let next = distance
            .iter()
            .position(|d| {
                pick -= d;
                pick <= 0.0
            })
            .unwrap_or(unit.len() - 1);
        centres.push(unit[next].to_vec());
    }

    let mut assignment = vec![0; unit.len()];
    for _ in 0..KMEANS_ROUNDS {
        let next: Vec<usize> = unit
            .iter()
            .map(|v| {
                (0..centres.len())
                    .max_by(|a, b| dot(&centres[*a], v).total_cmp(&dot(&centres[*b], v)))
                    .expect("at least one centre")
            })
            .collect();
        let settled = next == assignment;
        assignment = next;
        for (c, centre) in centres.iter_mut().enumerate() {
            let mut sum = vec![0.0; centre.len()];
            for (v, _) in unit.iter().zip(&assignment).filter(|(_, a)| **a == c) {
                for (s, x) in sum.iter_mut().zip(v.iter()) {
                    *s += x;
                }
            }
            if sum.iter().any(|s| *s != 0.0) {
                *centre = normalized(&sum);
            }
        }
        if settled {
            break;
        }
    }
    assignment
}

fn describe_clusters(embedded: &[(&MeetingRow, Vec<f32>)], assignment: &[usize]) -> Vec<Cluster> {
    let title_words = |row: &MeetingRow| -> Vec<String> {
        let mut ws: Vec<String> = words(&row.title)
            .into_iter()
            .filter(|w| is_content_word(w))
            .collect();
        ws.sort();
        ws.dedup();
        ws
    };
    let mut overall: HashMap<String, usize> = HashMap::new();
    for (row, _) in embedded {
        for w in title_words(row) {
            *overall.entry(w).or_default() += 1;
        }
    }

    let mut groups: BTreeMap<usize, Vec<&MeetingRow>> = BTreeMap::new();
    for ((row, _), c) in embedded.iter().zip(assignment) {
        groups.entry(*c).or_default().push(row);
    }
    let mut clusters: Vec<Cluster> = groups
        .into_iter()
        .map(|(id, rows)| {
            let mut here: HashMap<String, usize> = HashMap::new();
            for row in &rows {
                for w in title_words(row) {
                    *here.entry(w).or_default() += 1;
                }
            }
            let n = rows.len() as f64;
            let total = embedded.len() as f64;
            let mut scored: Vec<(String, f64)> = here
                .into_iter()
                .filter(|(_, c)| *c >= 2)
                .map(|(w, c)| {
                    let lift = (c as f64 / n) / (overall[&w] as f64 / total);
                    (w, c as f64 * lift.ln())
                })
                .filter(|(_, score)| *score > 0.0)
                .collect();
            scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            Cluster {
                id,
                label: scored.into_iter().take(3).map(|(w, _)| w).collect(),
                meetings: rows.len(),
                first: rows.iter().map(|r| r.created_at).min().expect("non-empty"),
                last: rows.iter().map(|r| r.created_at).max().expect("non-empty"),
            }
        })
        .collect();
    clusters.sort_by(|a, b| b.meetings.cmp(&a.meetings).then(a.id.cmp(&b.id)));
    clusters
}

/// The first two principal components of the vectors, by power iteration.
fn projection(unit: &[&[f32]]) -> (Vec<f32>, Vec<f32>) {
    let Some(first) = unit.first() else {
        return (vec![], vec![]);
    };
    let dims = first.len();
    let mut mean = vec![0.0; dims];
    for v in unit {
        for (m, x) in mean.iter_mut().zip(v.iter()) {
            *m += x / unit.len() as f32;
        }
    }
    let centred: Vec<Vec<f32>> = unit
        .iter()
        .map(|v| v.iter().zip(&mean).map(|(x, m)| x - m).collect())
        .collect();

    // Start from the meeting farthest from the mean: never orthogonal to
    // the data, and the same every time.
    let start = centred
        .iter()
        .max_by(|a, b| dot(a, a).total_cmp(&dot(b, b)))
        .expect("non-empty");
    let component = |orthogonal_to: Option<&[f32]>| -> Vec<f32> {
        let mut c = normalized(start);
        for _ in 0..POWER_ROUNDS {
            let scores: Vec<f32> = centred.iter().map(|v| dot(v, &c)).collect();
            let mut next = vec![0.0; dims];
            for (v, s) in centred.iter().zip(&scores) {
                for (n, x) in next.iter_mut().zip(v) {
                    *n += x * s;
                }
            }
            if let Some(o) = orthogonal_to {
                let along = dot(&next, o);
                for (n, x) in next.iter_mut().zip(o) {
                    *n -= along * x;
                }
            }
            c = normalized(&next);
        }
        c
    };
    let first = component(None);
    let second = component(Some(&first));
    (
        centred.iter().map(|v| dot(v, &first)).collect(),
        centred.iter().map(|v| dot(v, &second)).collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::fixtures::untranscribed;

    fn meeting(id: &str, created: &str, series: Option<&str>, title: &str) -> MeetingRow {
        let mut row = untranscribed(created);
        row.id = id.to_string();
        row.series = series.map(str::to_string);
        row.title = title.to_string();
        row
    }

    fn vectors(entries: &[(&str, [f32; 3])]) -> HashMap<String, Vec<f32>> {
        entries
            .iter()
            .map(|(id, v)| (id.to_string(), v.to_vec()))
            .collect()
    }

    #[test]
    fn meetings_without_an_embedding_are_left_out() {
        let rows = [
            meeting("a", "2026-03-02T10:00:00Z", None, "Budget"),
            meeting("b", "2026-03-03T10:00:00Z", None, "Hiring"),
        ];

        let r = report(&rows, &vectors(&[("a", [1.0, 0.0, 0.0])]));
        assert_eq!((r.meetings, r.embedded), (2, 1));
    }

    #[test]
    fn a_series_saying_the_same_thing_each_time_rehashes() {
        let rows = [
            meeting("a", "2026-03-02T10:00:00Z", Some("s"), "Sync"),
            meeting("b", "2026-03-09T10:00:00Z", Some("s"), "Sync"),
            meeting("c", "2026-03-16T10:00:00Z", Some("s"), "Sync"),
        ];
        let v = vectors(&[
            ("a", [1.0, 0.0, 0.0]),
            ("b", [1.0, 0.0, 0.0]),
            ("c", [0.0, 1.0, 0.0]),
        ]);

        let r = &report(&rows, &v).rehash[0];
        assert_eq!(
            (r.pairs, r.typical_similarity, r.latest_similarity),
            (2, 0.0, 0.0)
        );
    }

    #[test]
    fn the_meeting_least_like_the_others_is_the_first_outlier() {
        let rows = [
            meeting("a", "2026-03-02T10:00:00Z", None, "Budget"),
            meeting("b", "2026-03-03T10:00:00Z", None, "Budget again"),
            meeting("c", "2026-03-04T10:00:00Z", None, "Offsite"),
        ];
        let v = vectors(&[
            ("a", [1.0, 0.1, 0.0]),
            ("b", [1.0, 0.0, 0.0]),
            ("c", [0.0, 0.0, 1.0]),
        ]);

        assert_eq!(report(&rows, &v).outliers[0].id, "c");
    }

    #[test]
    fn series_with_the_same_content_are_twins() {
        let mut rows = Vec::new();
        let mut v = Vec::new();
        for (n, series, vector) in [
            (0, "x", [1.0, 0.0, 0.0]),
            (1, "x", [1.0, 0.0, 0.0]),
            (2, "x", [1.0, 0.0, 0.0]),
            (3, "y", [1.0, 0.05, 0.0]),
            (4, "y", [1.0, 0.0, 0.05]),
            (5, "y", [1.0, 0.0, 0.0]),
            (6, "z", [0.0, 1.0, 0.0]),
            (7, "z", [0.0, 1.0, 0.0]),
            (8, "z", [0.0, 1.0, 0.0]),
        ] {
            let id = format!("m{n}");
            rows.push(meeting(
                &id,
                &format!("2026-03-0{}T10:00:00Z", n + 1),
                Some(series),
                series,
            ));
            v.push((id, vector));
        }
        let v: HashMap<String, Vec<f32>> = v.into_iter().map(|(id, x)| (id, x.to_vec())).collect();

        let twin = &report(&rows, &v).twins[0];
        assert_eq!((twin.a.as_str(), twin.b.as_str()), ("x", "y"));
    }

    #[test]
    fn series_with_the_same_title_are_not_twins() {
        let mut rows = Vec::new();
        let mut v = HashMap::new();
        for (n, series) in ["x", "x", "x", "y", "y", "y"].iter().enumerate() {
            let id = format!("m{n}");
            rows.push(meeting(
                &id,
                &format!("2026-03-0{}T10:00:00Z", n + 1),
                Some(series),
                "Standup",
            ));
            v.insert(id, vec![1.0, 0.0, 0.0]);
        }

        assert_eq!(report(&rows, &v).twins, vec![]);
    }

    #[test]
    fn clusters_separate_distinct_topics_and_are_labelled_by_title_words() {
        let mut rows = Vec::new();
        let mut v = HashMap::new();
        for n in 0..40 {
            let (title, vector) = if n % 2 == 0 {
                ("Budget forecast", vec![1.0, 0.0, 0.01 * n as f32])
            } else {
                ("Hiring loop", vec![0.0, 1.0, 0.01 * n as f32])
            };
            let id = format!("m{n:02}");
            let created = format!("2026-03-{:02}T{:02}:00:00Z", 1 + n / 20, n % 20);
            rows.push(meeting(&id, &created, None, title));
            v.insert(id, vector);
        }

        let r = report(&rows, &v);
        let budget = r.map.iter().find(|m| m.id == "m00").unwrap().cluster;
        let hiring = r.map.iter().find(|m| m.id == "m01").unwrap().cluster;
        let label = &r.clusters.iter().find(|c| c.id == budget).unwrap().label;
        assert_eq!(
            (budget != hiring, label.contains(&"budget".to_string())),
            (true, true)
        );
    }

    #[test]
    fn the_map_spreads_distinct_meetings_apart() {
        let rows = [
            meeting("a", "2026-03-02T10:00:00Z", None, "A"),
            meeting("b", "2026-03-03T10:00:00Z", None, "B"),
        ];
        let v = vectors(&[("a", [1.0, 0.0, 0.0]), ("b", [0.0, 1.0, 0.0])]);

        let map = report(&rows, &v).map;
        assert!((map[0].x - map[1].x).abs() > 1.0, "{map:?}");
    }
}
