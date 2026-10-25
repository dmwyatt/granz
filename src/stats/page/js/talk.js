// Conversation shape. Mirrors src/stats/talk.rs: shares are pooled over
// characters, back and forth is handoffs per hour of talk, questions are
// utterances ending in "?", and tenths split each single-session meeting
// into ten equal parts.

const sideOf = (v) => (v.voice === "me" ? "me" : v.voice === "unknown" ? null : "others");

function talkFacts(m) {
  const voices = m.transcript ? m.transcript.voices : [];
  const sum = (side, field) => d3.sum(voices.filter((v) => sideOf(v) === side), (v) => v[field]);
  return {
    myUtterances: sum("me", "utterances"),
    othersUtterances: sum("others", "utterances"),
    myQuestions: sum("me", "questions"),
    othersQuestions: sum("others", "questions"),
    handoffsPerHour: m.spokenMinutes > 0 ? m.transcript.handoffs / (m.spokenMinutes / 60) : null,
  };
}

function shareGroup(ms) {
  const facts = ms.map(talkFacts);
  const rate = (q, n) => (n > 0 ? q / n : null);
  return {
    meetings: ms.length,
    myShare: share(d3.sum(ms, (m) => m.myChars), d3.sum(ms, (m) => m.othersChars)),
    handoffsPerHour: d3.median(facts, (f) => f.handoffsPerHour),
    myQuestions: rate(d3.sum(facts, (f) => f.myQuestions), d3.sum(facts, (f) => f.myUtterances)),
    othersQuestions: rate(d3.sum(facts, (f) => f.othersQuestions), d3.sum(facts, (f) => f.othersUtterances)),
  };
}

function tenths(ms) {
  const mine = new Array(10).fill(0);
  const theirs = new Array(10).fill(0);
  for (const m of ms.filter((m) => m.sessions.length === 1)) {
    const voices = m.transcript.voices;
    const minutes = d3.max(voices, (v) => v.chars_by_minute.length) || 0;
    for (const v of voices) {
      const side = sideOf(v);
      if (!side) continue;
      v.chars_by_minute.forEach((chars, minute) => {
        (side === "me" ? mine : theirs)[Math.floor((minute * 10) / minutes)] += chars;
      });
    }
  }
  return mine.map((m, i) => ({ tenth: i + 1, share: share(m, theirs[i]) }));
}

section("talk", "Conversation",
  "How much you talk and how the talk flows, from the two audio channels: your microphone, and everyone else. It covers every meeting with a transcript.",
  (meetings, body) => {
    const transcribed = meetings.filter((m) => m.transcript);
    shareOverTime(body, transcribed);
    const grid = el("div", { class: "grid" });
    body.append(grid);
    shareBySize(grid, transcribed);
    shareByTenth(grid, transcribed);
    backAndForth(grid, transcribed);
    questionRates(grid, transcribed);
    meetingStrips(body, transcribed);
    namedStrip(body, transcribed);
    silentMeetings(body, transcribed);
  });

function shareOverTime(body, meetings) {
  const months = d3.groups(meetings, (m) => d3.timeMonth.floor(m.date))
    .map(([month, ms]) => ({ month, ...shareGroup(ms) }))
    .filter((d) => d.myShare != null)
    .sort((a, b) => a.month - b.month);
  const dots = meetings.map((m) => ({ m, share: share(m.myChars, m.othersChars) })).filter((d) => d.share != null);

  figure(body, {
    title: "Your share of the talk",
    caption: "Each dot is a meeting; the line is each month's share, pooled over its meetings.",
    plot: (width) => months.length ? Plot.plot({
      ...plotDefaults(width),
      height: 260,
      x: { type: "time", label: null },
      y: { domain: [0, 1], tickFormat: "%", label: "your share" },
      marks: [
        gridY(),
        Plot.dot(dots, { x: (d) => d.m.date, y: "share", r: 2, fill: seriesColor(0), fillOpacity: 0.25,
          tip: { channels: { meeting: (d) => d.m.title }, format: { x: (d) => isoDay(d), y: (v) => fmtPct(v) } } }),
        Plot.lineY(months, { x: (d) => d3.timeDay.offset(d.month, 14), y: "myShare", stroke: seriesColor(0), strokeWidth: 2 }),
        ...eraMarks(d3.extent(meetings, (m) => m.date)),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Month", value: (d) => d3.timeFormat("%Y-%m")(d.month) },
        { label: "Meetings", value: (d) => d.meetings, num: true },
        { label: "Your share", value: (d) => fmtPct(d.myShare), num: true },
      ],
      rows: [...months].reverse(),
    },
  });
}

function shareBySize(parent, meetings) {
  const rows = SIZE_BUCKETS.map((bucket) => ({ bucket, ...shareGroup(meetings.filter((m) => sizeBucket(m.invited) === bucket)) }))
    .filter((d) => d.meetings > 0);
  figure(parent, {
    title: "Your share by meeting size",
    caption: "Invitees who did not decline, not counting you.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 220,
      x: { domain: rows.map((d) => d.bucket), label: "invitees" },
      y: { domain: [0, 1], tickFormat: "%", label: "your share" },
      marks: [
        gridY(),
        Plot.barY(rows, { x: "bucket", y: "myShare", fill: seriesColor(0), insetLeft: 8, insetRight: 8,
          tip: { channels: { meetings: "meetings" }, format: { y: (v) => fmtPct(v) } } }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Invitees", value: (d) => d.bucket },
        { label: "Meetings", value: (d) => d.meetings, num: true },
        { label: "Your share", value: (d) => fmtPct(d.myShare), num: true },
      ],
      rows,
    },
  });
}

function shareByTenth(parent, meetings) {
  const rows = tenths(meetings).filter((d) => d.share != null);
  figure(parent, {
    title: "When in a meeting you speak",
    caption: "Your share of the talk in each tenth of a meeting, first to last.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 220,
      x: { domain: d3.range(1, 11), label: "tenth of the meeting" },
      y: { tickFormat: "%", label: "your share", domain: [0, Math.max(0.5, d3.max(rows, (d) => d.share))] },
      marks: [
        gridY(),
        Plot.barY(rows, { x: "tenth", y: "share", fill: seriesColor(0), insetLeft: 4, insetRight: 4,
          tip: { format: { y: (v) => fmtPct(v) } } }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Tenth", value: (d) => d.tenth, num: true },
        { label: "Your share", value: (d) => fmtPct(d.share), num: true },
      ],
      rows,
    },
  });
}

function backAndForth(parent, meetings) {
  const dots = meetings.map((m) => ({ m, rate: talkFacts(m).handoffsPerHour, share: share(m.myChars, m.othersChars) }))
    .filter((d) => d.rate != null && d.share != null);
  figure(parent, {
    title: "Conversation or presentation",
    caption: "Each meeting by how often the talk passed between you and everyone else, against your share. Bottom left is listening to a presentation.",
    plot: (width) => dots.length ? Plot.plot({
      ...plotDefaults(width),
      height: 260,
      x: { label: "handoffs per hour", grid: true },
      y: { domain: [0, 1], tickFormat: "%", label: "your share", grid: true },
      marks: [
        Plot.dot(dots, { x: "rate", y: "share", r: 3, fill: seriesColor(0), fillOpacity: 0.5,
          tip: { channels: { meeting: (d) => d.m.title, day: (d) => isoDay(d.m.date) },
            format: { x: (v) => `${Math.round(v)}/h`, y: (v) => fmtPct(v) } } }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Meeting", value: (d) => d.m.title },
        { label: "Day", value: (d) => isoDay(d.m.date) },
        { label: "Handoffs per hour", value: (d) => Math.round(d.rate), num: true },
        { label: "Your share", value: (d) => fmtPct(d.share), num: true },
      ],
      rows: dots.slice().sort((a, b) => b.m.date - a.m.date),
    },
  });
}

function questionRates(parent, meetings) {
  const sides = ["You", "Everyone else"];
  const rows = d3.groups(meetings, (m) => d3.timeMonth.floor(m.date))
    .flatMap(([month, ms]) => {
      const g = shareGroup(ms);
      return [{ month, side: "You", rate: g.myQuestions }, { month, side: "Everyone else", rate: g.othersQuestions }];
    })
    .filter((d) => d.rate != null)
    .sort((a, b) => a.month - b.month);
  figure(parent, {
    title: "How often each side asks questions",
    caption: "Share of utterances ending in a question mark, per month.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 220,
      x: { type: "time", label: null },
      y: { tickFormat: ".0%", label: "questions", zero: true },
      color: { domain: sides, range: [seriesColor(0), seriesColor(1)], legend: true },
      marks: [
        gridY(),
        Plot.lineY(rows, { x: "month", y: "rate", stroke: "side", strokeWidth: 2,
          tip: { format: { x: (d) => d3.timeFormat("%B %Y")(d), y: (v) => `${(v * 100).toFixed(1)}%` } } }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Month", value: (d) => d3.timeFormat("%Y-%m")(d.month) },
        { label: "Side", value: (d) => d.side },
        { label: "Questions", value: (d) => `${(d.rate * 100).toFixed(1)}%`, num: true },
      ],
      rows: [...rows].reverse(),
    },
  });
}

// Per-minute cells for one meeting, one lane per entry of `lanes`
// ({ name, keep(voice) }), shaded by the minute's share of the meeting's
// busiest minute.
function stripCells(m, lanes) {
  const voices = m.transcript.voices;
  const cells = lanes.flatMap((lane) => {
    const binned = voices.filter(lane.keep).map((v) => v.chars_by_minute);
    const minutes = d3.max(binned, (b) => b.length) || 0;
    return d3.range(minutes).map((minute) => ({
      meeting: `${isoDay(m.date)} ${m.title}`, lane: lane.name, minute,
      chars: d3.sum(binned, (b) => b[minute] || 0),
    }));
  }).filter((c) => c.chars > 0);
  const busiest = d3.max(cells, (c) => c.chars) || 1;
  return cells.map((c) => ({ ...c, intensity: c.chars / busiest }));
}

function meetingStrips(body, meetings) {
  const recent = meetings.filter((m) => m.sessions.length === 1).sort((a, b) => b.date - a.date).slice(0, 12);
  const lanes = [
    { name: "You", keep: (v) => v.voice === "me" },
    { name: "Others", keep: (v) => sideOf(v) === "others" },
  ];
  const cells = recent.flatMap((m) => stripCells(m, lanes));
  const rows = recent.map((m) => `${isoDay(m.date)} ${m.title}`);

  figure(body, {
    title: "Who talks when, meeting by meeting",
    caption: "The 12 most recent meetings in the selection (pick a series above to compare its meetings). Each column is a minute; darker means more said in it.",
    plot: (width) => cells.length ? Plot.plot({
      ...plotDefaults(width),
      height: rows.length * 44 + 40,
      marginLeft: 290,
      x: { label: "minutes from the first word" },
      y: { domain: ["You", "Others"], label: null, tickSize: 0 },
      fy: { domain: rows, axis: null, padding: 0.25 },
      color: { domain: ["You", "Others"], range: [seriesColor(0), seriesColor(1)], legend: true },
      marks: [
        Plot.text(rows, { fy: (d) => d, text: (d) => (d.length > 36 ? `${d.slice(0, 35)}…` : d),
          frameAnchor: "left", textAnchor: "end", dx: -52, fill: token("--ink-2") }),
        Plot.rect(cells, { x1: "minute", x2: (d) => d.minute + 1, y: "lane", fy: "meeting", fill: "lane",
          fillOpacity: (d) => 0.15 + 0.85 * d.intensity, insetTop: 1, insetBottom: 1,
          tip: { channels: { characters: "chars" }, format: { x1: (v) => `minute ${v}`, x2: null, fillOpacity: null, fy: null } } }),
      ],
    }) : null,
  });
}

function namedStrip(body, meetings) {
  const m = meetings.filter((m) => m.namedSpeakers > 0 && m.sessions.length === 1).sort((a, b) => b.date - a.date)[0];
  if (!m) return;
  const names = m.transcript.voices.filter((v) => v.voice === "named").sort((a, b) => b.chars - a.chars).map((v) => v.name);
  const lanes = [
    { name: "You", keep: (v) => v.voice === "me" },
    ...names.slice(0, 7).map((name) => ({ name, keep: (v) => v.voice === "named" && v.name === name })),
    { name: "Unnamed or others", keep: (v) => v.voice === "others" || (v.voice === "named" && !names.slice(0, 7).includes(v.name)) },
  ];
  const cells = stripCells(m, lanes);
  const domain = lanes.map((l) => l.name).filter((n) => cells.some((c) => c.lane === n));

  figure(body, {
    title: "One meeting, speaker by speaker",
    caption: `The most recent meeting in the selection with speaker names: ${m.title}, ${isoDay(m.date)}.`,
    plot: (width) => cells.length ? Plot.plot({
      ...plotDefaults(width),
      height: domain.length * 26 + 50,
      marginLeft: 140,
      x: { label: "minutes from the first word" },
      y: { domain, label: null, tickSize: 0 },
      marks: [
        Plot.rect(cells, { x1: "minute", x2: (d) => d.minute + 1, y: "lane", fill: seriesColor(0),
          fillOpacity: (d) => 0.15 + 0.85 * d.intensity, insetTop: 2, insetBottom: 2,
          tip: { channels: { characters: "chars" }, format: { x1: (v) => `minute ${v}`, x2: null, fillOpacity: null } } }),
      ],
    }) : null,
  });
}

function silentMeetings(body, meetings) {
  const silent = meetings.filter((m) => m.myChars === 0 && m.othersChars > 0);
  const micOnly = meetings.filter((m) => m.myChars > 0 && m.othersChars === 0);
  const rows = [...silent.map((m) => ({ m, kind: "You never spoke" })), ...micOnly.map((m) => ({ m, kind: "Only your microphone" }))]
    .sort((a, b) => b.m.date - a.m.date);
  const card = figure(body, {
    title: "Silent meetings",
    caption: `${fmtInt(silent.length)} meetings where you never spoke, and ${fmtInt(micOnly.length)} with only your microphone, which are often in person with everyone on that one microphone.`,
  });
  if (rows.length) {
    card.append(el("details", {}, el("summary", {}, `Show ${rows.length} meetings`),
      el("div", { class: "table-wrap" }, renderTable({
        columns: [
          { label: "Day", value: (d) => isoDay(d.m.date) },
          { label: "Meeting", value: (d) => d.m.title },
          { label: "Kind", value: (d) => d.kind },
        ],
        rows,
      }))));
  }
}
