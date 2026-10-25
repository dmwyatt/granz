// Words. Mirrors src/stats/words.rs. Tics come per meeting, so they follow
// the filters; signature words and new terms were computed over every
// meeting in this file.

section("words", "Words",
  "What gets said. Granola removes most single-word fillers, so the tics here are the multi-word ones that survive.",
  (meetings, body) => {
    ticsOverTime(body, meetings);
    const grid = el("div", { class: "grid" });
    body.append(grid);
    signatureWords(grid);
    newTerms(grid);
    mentionedWhileAbsent(body, meetings);
  });

// Mirrors src/stats/mentions.rs: a full name said in a meeting the person
// was not invited to, or declined.
function mentionedWhileAbsent(body, meetings) {
  const byPerson = new Map();
  for (const m of meetings) {
    const said = DATA.words.meeting_mentions[m.id];
    if (!said) continue;
    const invited = new Set(attending(m).map((i) => i.email));
    for (const [email, count] of Object.entries(said)) {
      if (invited.has(email)) continue;
      const p = byPerson.get(email) || { email, name: personName(email), meetings: 0, mentions: 0, last: m.date };
      p.meetings += 1;
      p.mentions += count;
      if (m.date > p.last) p.last = m.date;
      byPerson.set(email, p);
    }
  }
  const rows = [...byPerson.values()].sort((a, b) => b.meetings - a.meetings);
  const top = rows.slice(0, 15);
  figure(body, {
    title: "Talked about while not invited",
    caption: "Meetings where someone's full name was said and they were not invited, or declined. Full names only: first names alone are too often ordinary words or someone else.",
    plot: (width) => top.length ? Plot.plot({
      ...plotDefaults(width),
      height: 24 * top.length + 40,
      marginLeft: 150,
      x: { label: "meetings", grid: true },
      y: { domain: top.map((p) => p.name), label: null },
      marks: [
        Plot.barX(top, { x: "meetings", y: "name", fill: seriesColor(0), insetTop: 4, insetBottom: 4,
          tip: { channels: { mentions: "mentions", last: (d) => isoDay(d.last) } } }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Person", value: (d) => d.name },
        { label: "Meetings", value: (d) => d.meetings, num: true },
        { label: "Mentions", value: (d) => d.mentions, num: true },
        { label: "Last", value: (d) => isoDay(d.last) },
      ],
      rows,
    },
  });
}

function ticsOverTime(body, meetings) {
  const tics = DATA.words.tics;
  const byMonth = d3.rollups(meetings.filter((m) => DATA.words.meeting_tics[m.id]),
    (ms) => {
      const counts = tics.map((_, i) => d3.sum(ms, (m) => DATA.words.meeting_tics[m.id].counts[i]));
      return { words: d3.sum(ms, (m) => DATA.words.meeting_tics[m.id].my_words), counts };
    },
    (m) => d3.timeMonth.floor(m.date)).sort((a, b) => a[0] - b[0]);
  const totals = tics.map((_, i) => d3.sum(byMonth, ([, v]) => v.counts[i]));
  const shown = tics.map((t, i) => [t, totals[i]]).filter(([, n]) => n > 0)
    .sort((a, b) => b[1] - a[1]).slice(0, 4).map(([t]) => t);
  const rows = byMonth.flatMap(([month, v]) => shown.map((tic) => ({
    month, tic, rate: v.words ? (v.counts[tics.indexOf(tic)] * 1000) / v.words : 0, words: v.words,
  })));

  figure(body, {
    title: "Your verbal tics",
    caption: "Times per thousand of your words, per month, for your four most frequent.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 240,
      x: { type: "time", label: null },
      y: { label: "per thousand words", zero: true },
      color: { domain: shown, range: shown.map((_, i) => seriesColor(i)), legend: true },
      marks: [
        gridY(),
        Plot.lineY(rows, { x: "month", y: "rate", stroke: "tic", strokeWidth: 2,
          tip: { channels: { "your words": "words" }, format: { x: (d) => d3.timeFormat("%B %Y")(d), y: (v) => v.toFixed(1) } } }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Month", value: (d) => d3.timeFormat("%Y-%m")(d[0]) },
        { label: "Your words", value: (d) => fmtInt(d[1].words), num: true },
        ...tics.map((t, i) => ({ label: t, value: (d) => (d[1].words ? ((d[1].counts[i] * 1000) / d[1].words).toFixed(1) : "–"), num: true })),
      ],
      rows: [...byMonth].reverse(),
    },
  });
}

function signatureWords(parent) {
  const rows = DATA.words.signature.slice(0, 20);
  figure(parent, {
    title: "Words you say more than anyone else",
    caption: "Scored by a log-odds ratio against everyone else's words; above about 2 is unlikely to be chance. Over every meeting in this file.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 22 * rows.length + 40,
      marginLeft: 110,
      x: { label: "score", grid: true },
      y: { domain: rows.map((d) => d.word), label: null },
      marks: [
        Plot.barX(rows, { x: "score", y: "word", fill: seriesColor(0), insetTop: 3, insetBottom: 3,
          tip: { channels: { you: "mine", others: "others" }, format: { x: (v) => v.toFixed(1) } } }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Word", value: (d) => d.word },
        { label: "You", value: (d) => fmtInt(d.mine), num: true },
        { label: "Everyone else", value: (d) => fmtInt(d.others), num: true },
        { label: "Score", value: (d) => d.score.toFixed(1), num: true },
      ],
      rows: DATA.words.signature,
    },
  });
}

function newTerms(parent) {
  const terms = DATA.words.emerging.slice(0, 8);
  const rows = terms.flatMap((t) => t.monthly.map(([month, n]) => ({ term: t.term, month: new Date(month + "T00:00:00"), n })));
  figure(parent, {
    title: "Terms new to your meetings",
    caption: "Words first heard after the first tenth of all the words in this file, by mentions per month. Over every meeting in this file.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 300,
      x: { type: "time", label: null },
      y: { axis: null },
      color: { domain: terms.map((t) => t.term), range: terms.map((_, i) => seriesColor(i)), legend: true },
      marks: [
        Plot.areaY(rows, Plot.stackY({ offset: "wiggle", order: "inside-out" },
          { x: "month", y: "n", fill: "term", curve: "basis", stroke: token("--surface"), strokeWidth: 1,
            tip: { format: { x: (d) => d3.timeFormat("%B %Y")(d) } } })),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Term", value: (d) => d.term },
        { label: "Mentions", value: (d) => fmtInt(d.mentions), num: true },
        { label: "First heard", value: (d) => d.first_month.slice(0, 7) },
        { label: "Peak", value: (d) => d.peak_month.slice(0, 7) },
      ],
      rows: DATA.words.emerging,
    },
  });
}
