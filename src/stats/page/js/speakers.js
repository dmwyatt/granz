// Named speakers. Mirrors src/stats/speakers.rs: only meetings with at
// least one named speaker count; a heard name is matched to the invitee
// with the same name, else to the one invitee sharing its first name.

const nameKey = (name) => name.split(/\s+/).filter(Boolean).join(" ").toLowerCase();
const firstName = (key) => key.split(" ")[0];

function resolveSpeaker(heard, invitees) {
  const k = nameKey(heard);
  const keys = invitees.map(nameKey);
  if (keys.includes(k)) return k;
  const same = keys.filter((i) => firstName(i) === firstName(k));
  return same.length === 1 ? same[0] : k;
}

function speakerRollups(meetings) {
  const people = new Map();
  const spellings = new Map();
  const get = (k) => {
    if (!people.has(k)) people.set(k, { key: k, spokeIn: 0, chars: 0, heardTotal: 0, invitedTo: 0, invitedNotHeard: 0, heardNotInvited: 0, youAnswered: 0, answeredYou: 0 });
    return people.get(k);
  };
  const spell = (k, name) => {
    const s = spellings.get(k) || new Map();
    s.set(name, (s.get(name) || 0) + 1);
    spellings.set(k, s);
  };
  for (const m of meetings) {
    const voices = m.transcript.voices;
    const total = d3.sum(voices, (v) => v.chars);
    const invitees = attending(m).map((i) => i.name).filter(Boolean);
    const invited = new Set(invitees.map(nameKey));
    invitees.forEach((n) => spell(nameKey(n), n));
    const heard = new Map();
    for (const v of voices.filter((v) => v.voice === "named")) {
      const k = resolveSpeaker(v.name, invitees);
      spell(k, v.name);
      heard.set(k, (heard.get(k) || 0) + v.chars);
    }
    for (const [k, chars] of heard) {
      const p = get(k);
      p.spokeIn += 1;
      p.chars += chars;
      p.heardTotal += total;
      if (!invited.has(k)) p.heardNotInvited += 1;
    }
    for (const k of invited) {
      const p = get(k);
      p.invitedTo += 1;
      if (!heard.has(k)) p.invitedNotHeard += 1;
    }
    for (const t of m.transcript.turns) {
      if (t.from.voice === "named" && t.to.voice === "me") get(resolveSpeaker(t.from.name, invitees)).youAnswered += t.count;
      if (t.from.voice === "me" && t.to.voice === "named") get(resolveSpeaker(t.to.name, invitees)).answeredYou += t.count;
    }
  }
  return [...people.values()].map((p) => ({
    ...p,
    name: [...(spellings.get(p.key) || new Map([[p.key, 1]]))].sort((a, b) => b[1] - a[1])[0][0],
    shareWhenHeard: share(p.chars, p.heardTotal - p.chars),
  })).sort((a, b) => b.chars - a.chars);
}

function evennessOf(m) {
  const chars = m.transcript.voices.filter((v) => (v.voice === "me" || v.voice === "named") && v.chars > 0).map((v) => v.chars);
  if (chars.length < 2) return null;
  const total = d3.sum(chars);
  const entropy = d3.sum(chars, (c) => -(c / total) * Math.log(c / total));
  return entropy / Math.log(chars.length);
}

section("speakers", "Named speakers",
  NAMED_SINCE
    ? `Granola has named the speakers on the other audio channel since ${isoDay(NAMED_SINCE)}, and never for older meetings, so this section covers only meetings with speaker names. Two people on one microphone come out as one speaker.`
    : "Granola has not named any speakers in these meetings.",
  (meetings, body) => {
    const named = meetings.filter((m) => m.namedSpeakers > 0);
    if (!named.length) {
      body.append(el("p", { class: "note" }, "No meetings with speaker names in this selection."));
      return;
    }
    const people = speakerRollups(named);
    invitedAgainstHeard(body, people);
    const grid = el("div", { class: "grid" });
    body.append(grid);
    airtime(grid, people);
    youAnswer(grid, people);
    turnMatrix(body, named, people);
    const grid2 = el("div", { class: "grid" });
    body.append(grid2);
    seriesDominance(grid2, named);
    evennessOverTime(grid2, named);
  });

function invitedAgainstHeard(body, people) {
  const kinds = ["Invited and heard", "Invited, never heard", "Heard, not invited"];
  const top = people.slice().sort((a, b) => (b.invitedTo + b.heardNotInvited) - (a.invitedTo + a.heardNotInvited)).slice(0, 20);
  const rows = top.flatMap((p) => [
    { name: p.name, kind: kinds[0], meetings: p.invitedTo - p.invitedNotHeard },
    { name: p.name, kind: kinds[1], meetings: p.invitedNotHeard },
    { name: p.name, kind: kinds[2], meetings: p.heardNotInvited },
  ]).filter((r) => r.meetings > 0);

  figure(body, {
    title: "Invited against heard",
    caption: "Meetings with speaker names, for the 20 people in the most of them: invited and heard, invited but never heard, and heard without an invitation.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 24 * top.length + 60,
      marginLeft: 150,
      x: { label: "meetings", grid: true },
      y: { domain: top.map((p) => p.name), label: null },
      color: { domain: kinds, range: [seriesColor(0), token("--other"), seriesColor(1)], legend: true },
      marks: [
        Plot.barX(rows, { x: "meetings", y: "name", fill: "kind", order: kinds, insetTop: 4, insetBottom: 4,
          stroke: token("--surface"), strokeWidth: 1, tip: true }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Person", value: (d) => d.name },
        { label: "Heard in", value: (d) => d.spokeIn, num: true },
        { label: "Invited to", value: (d) => d.invitedTo, num: true },
        { label: "Invited, never heard", value: (d) => d.invitedNotHeard, num: true },
        { label: "Heard, not invited", value: (d) => d.heardNotInvited, num: true },
      ],
      rows: people,
    },
  });
}

function airtime(parent, people) {
  const rows = people.filter((p) => p.spokeIn > 0).slice(0, 15);
  figure(parent, {
    title: "Who talks most when they are there",
    caption: "Each person's share of all the talk in the meetings they were heard in, for the 15 who talked most.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 24 * rows.length + 40,
      marginLeft: 140,
      x: { domain: [0, 1], tickFormat: "%", label: "share when heard", grid: true },
      y: { domain: rows.map((p) => p.name), label: null },
      marks: [
        Plot.barX(rows, { x: "shareWhenHeard", y: "name", fill: seriesColor(0), insetTop: 4, insetBottom: 4,
          tip: { channels: { meetings: "spokeIn" }, format: { x: (v) => fmtPct(v) } } }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Person", value: (d) => d.name },
        { label: "Heard in", value: (d) => d.spokeIn, num: true },
        { label: "Share when heard", value: (d) => fmtPct(d.shareWhenHeard), num: true },
      ],
      rows,
    },
  });
}

function youAnswer(parent, people) {
  const rows = people.filter((p) => p.youAnswered > 0).sort((a, b) => b.youAnswered - a.youAnswered).slice(0, 15);
  figure(parent, {
    title: "Who you answer most",
    caption: "Times you spoke right after each person, for the 15 you answered most.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 24 * rows.length + 40,
      marginLeft: 140,
      x: { label: "times", grid: true },
      y: { domain: rows.map((p) => p.name), label: null },
      marks: [
        Plot.barX(rows, { x: "youAnswered", y: "name", fill: seriesColor(0), insetTop: 4, insetBottom: 4,
          tip: { channels: { "answered you": "answeredYou" } } }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Person", value: (d) => d.name },
        { label: "You answered them", value: (d) => d.youAnswered, num: true },
        { label: "They answered you", value: (d) => d.answeredYou, num: true },
      ],
      rows: people.filter((p) => p.youAnswered + p.answeredYou > 0),
    },
  });
}

function turnMatrix(body, meetings, people) {
  const top = ["You", ...people.filter((p) => p.spokeIn > 0).slice(0, 11).map((p) => p.name)];
  const nameOf = new Map(people.map((p) => [p.key, p.name]));
  const counts = new Map();
  for (const m of meetings) {
    const invitees = attending(m).map((i) => i.name).filter(Boolean);
    const who = (v) => (v.voice === "me" ? "You" : v.voice === "named" ? nameOf.get(resolveSpeaker(v.name, invitees)) : null);
    for (const t of m.transcript.turns) {
      const from = who(t.from);
      const to = who(t.to);
      if (!top.includes(from) || !top.includes(to) || from === to) continue;
      const k = `${from}\n${to}`;
      counts.set(k, (counts.get(k) || 0) + t.count);
    }
  }
  const cells = [...counts].map(([k, count]) => { const [from, to] = k.split("\n"); return { from, to, count }; });

  figure(body, {
    title: "Who speaks after whom",
    caption: "Rows are the speaker, columns who spoke right after them, for you and the 11 named speakers who talked most.",
    plot: (width) => cells.length ? Plot.plot({
      ...plotDefaults(width),
      height: 26 * top.length + 120,
      marginLeft: 140,
      marginBottom: 90,
      x: { domain: top, label: "spoke next", tickRotate: -35 },
      y: { domain: top, label: null },
      color: { ...sequentialScale(), label: "times", legend: true },
      marks: [
        Plot.cell(cells, { x: "to", y: "from", fill: "count", inset: 1, rx: 2, tip: true }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Speaker", value: (d) => d.from },
        { label: "Spoke next", value: (d) => d.to },
        { label: "Times", value: (d) => d.count, num: true },
      ],
      rows: cells.sort((a, b) => b.count - a.count),
    },
  });
}

function seriesDominance(parent, meetings) {
  const rows = d3.groups(meetings.filter((m) => m.series), (m) => m.series).map(([id, ms]) => {
    const chars = new Map();
    for (const m of ms) {
      for (const v of m.transcript.voices) {
        const who = v.voice === "me" ? "You" : v.voice === "named" ? v.name : null;
        if (who) chars.set(who, (chars.get(who) || 0) + v.chars);
      }
    }
    const total = d3.sum(chars.values());
    const [top, topChars] = [...chars].sort((a, b) => b[1] - a[1])[0] || ["–", 0];
    return { title: SERIES_TITLES.get(id), meetings: ms.length, top, share: total ? topChars / total : null };
  }).sort((a, b) => b.meetings - a.meetings);
  const card = figure(parent, {
    title: "Who talks most in each series",
    caption: "Over each series' meetings with speaker names, you included.",
  });
  card.append(el("div", { class: "table-wrap" }, renderTable({
    columns: [
      { label: "Series", value: (d) => d.title },
      { label: "Meetings", value: (d) => d.meetings, num: true },
      { label: "Talks most", value: (d) => d.top },
      { label: "Their share", value: (d) => fmtPct(d.share), num: true },
    ],
    rows,
  })));
}

function evennessOverTime(parent, meetings) {
  const dots = meetings.map((m) => ({ m, evenness: evennessOf(m) })).filter((d) => d.evenness != null);
  const months = d3.groups(dots, (d) => d3.timeMonth.floor(d.m.date))
    .map(([month, ds]) => ({ month, evenness: d3.median(ds, (d) => d.evenness), meetings: ds.length }))
    .sort((a, b) => a.month - b.month);
  figure(parent, {
    title: "How evenly the talk is spread",
    caption: "1 when you and every named speaker talked equally, near 0 when one voice did nearly all of it. Dots are meetings; the line is the monthly median.",
    plot: (width) => dots.length ? Plot.plot({
      ...plotDefaults(width),
      height: 240,
      x: { type: "time", label: null },
      y: { domain: [0, 1], label: "evenness" },
      marks: [
        gridY(),
        Plot.dot(dots, { x: (d) => d.m.date, y: "evenness", r: 2.5, fill: seriesColor(0), fillOpacity: 0.35,
          tip: { channels: { meeting: (d) => d.m.title }, format: { x: (d) => isoDay(d), y: (v) => v.toFixed(2) } } }),
        Plot.lineY(months, { x: (d) => d3.timeDay.offset(d.month, 14), y: "evenness", stroke: seriesColor(0), strokeWidth: 2 }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Month", value: (d) => d3.timeFormat("%Y-%m")(d.month) },
        { label: "Meetings", value: (d) => d.meetings, num: true },
        { label: "Median evenness", value: (d) => d.evenness.toFixed(2), num: true },
      ],
      rows: [...months].reverse(),
    },
  });
}
