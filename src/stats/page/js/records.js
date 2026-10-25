// The record book and the year in review. Records mirror
// src/stats/records.rs; both follow the filters.

function recordsFor(meetings) {
  const transcribed = meetings.filter((m) => m.transcript);
  const best = (items, value) => items.reduce((top, item) => {
    const v = value(item);
    return v != null && (top == null || v > top.v) ? { item, v } : top;
  }, null);
  const monologue = (keep) => best(
    transcribed.flatMap((m) => m.transcript.voices
      .filter((v) => (v.voice === "me" || v.voice === "named") && keep(v))
      .map((v) => ({ m, v }))),
    (x) => x.v.longest_run);
  const days = dayLoads(meetings);
  const records = [];
  const add = (name, found, value, where) => {
    if (found) records.push({ name, value: value(found), where: where(found) });
  };
  const meetingWhere = (m) => `${m.title}, ${isoDay(m.date)}`;

  add("Longest meeting", best(transcribed, (m) => m.spokenMinutes), (f) => `${Math.round(f.v)} min`, (f) => meetingWhere(f.item));
  add("Longest monologue", monologue(() => true), (f) => `${fmtInt(f.v)} characters`,
    (f) => `${f.item.v.voice === "me" ? "You" : f.item.v.name}, in ${meetingWhere(f.item.m)}`);
  add("Your longest monologue", monologue((v) => v.voice === "me"), (f) => `${fmtInt(f.v)} characters`, (f) => meetingWhere(f.item.m));
  add("Biggest invite list", best(meetings, (m) => m.invited), (f) => `${f.v} invitees`, (f) => meetingWhere(f.item));
  add("Most speakers heard", best(transcribed, (m) => m.namedSpeakers || null), (f) => `${f.v} speakers`, (f) => meetingWhere(f.item));
  add("Most questions you asked", best(transcribed, (m) => (m.transcript.voices.find((v) => v.voice === "me") || {}).questions),
    (f) => `${f.v} questions`, (f) => meetingWhere(f.item));
  add("Most back and forth", best(transcribed, (m) => (m.spokenMinutes >= 10 ? m.transcript.handoffs / (m.spokenMinutes / 60) : null)),
    (f) => `${Math.round(f.v)} per hour`, (f) => meetingWhere(f.item));
  add("Busiest day", best(days, (d) => d.hours), (f) => `${f.v.toFixed(1)} h`, (f) => isoDay(f.item.day));
  add("Most meetings in a day", best(days, (d) => d.sessions.length), (f) => `${f.v} meetings`, (f) => isoDay(f.item.day));
  add("Longest back-to-back run", best(days, (d) => d.streak), (f) => `${f.v} meetings`, (f) => isoDay(f.item.day));
  add("Earliest start", best(days, (d) => -d.first), (f) => fmtClock(-f.v), (f) => isoDay(f.item.day));
  add("Latest finish", best(days, (d) => d.last), (f) => fmtClock(f.v), (f) => isoDay(f.item.day));
  return records;
}

section("records", "Record book", "The most of everything in the selection, and where each record was set.", (meetings, body) => {
  tiles(body, recordsFor(meetings).map((r) => ({ label: r.name, value: r.value, detail: r.where })));
});

section("years", "Year in review", "Each calendar year in the selection, at a glance.", (meetings, body) => {
  const years = d3.groups(meetings, (m) => m.date.getFullYear()).sort((a, b) => b[0] - a[0]);
  for (const [year, ms] of years) yearCard(body, year, ms);
});

function yearCard(body, year, meetings) {
  const transcribed = meetings.filter((m) => m.transcript);
  const hours = d3.sum(meetings, (m) => m.spokenMinutes) / 60;
  const months = d3.rollups(meetings, (v) => d3.sum(v, (m) => m.spokenMinutes) / 60, (m) => d3.timeMonth.floor(m.date))
    .sort((a, b) => b[1] - a[1]);
  const people = personRollups(meetings).sort((a, b) => b.hours - a.hours).slice(0, 5);
  const series = seriesRollups(meetings).slice(0, 5);
  const ones = personRollups(meetings).filter((p) => p.oneOnOnes > 0).sort((a, b) => b.oneOnOnes - a.oneOnOnes).slice(0, 3);
  const card = el("figure", { class: "card year" },
    el("h3", {}, String(year)),
    el("div", { class: "hero" }, `${fmtInt(hours)} hours`),
    el("figcaption", {}, `in ${fmtInt(meetings.length)} meetings, ${fmtInt(transcribed.length)} of them transcribed`));
  body.append(card);
  tiles(card, [
    { label: "Busiest month", value: months.length ? d3.timeFormat("%B")(months[0][0]) : "–", detail: months.length ? `${months[0][1].toFixed(1)} h` : null },
    { label: "Typical meeting", value: transcribed.length ? `${fmtInt(d3.median(transcribed, (m) => m.spokenMinutes))} min` : "–" },
    { label: "Your share", value: fmtPct(share(d3.sum(meetings, (m) => m.myChars), d3.sum(meetings, (m) => m.othersChars))), detail: "of what was said" },
    { label: "People", value: fmtInt(personRollups(meetings).length), detail: "invited to your meetings" },
  ]);
  const lists = el("div", { class: "grid year-lists" });
  card.append(lists);
  const list = (title, items, text) => lists.append(el("div", {},
    el("h4", {}, title),
    items.length ? el("ol", {}, items.map((i) => el("li", {}, text(i)))) : el("p", { class: "note" }, "None")));
  list("Most time with", people, (p) => `${p.name}, ${p.hours.toFixed(1)} h`);
  list("Biggest series", series, (s) => `${s.title}, ${s.hours.toFixed(1)} h`);
  list("Most one-on-ones", ones, (p) => `${p.name}, ${p.oneOnOnes}`);
}
