// Notes. Mirrors src/stats/notes.rs: action items are the bullets under
// "Next steps" or "Action items" headings in Granola's summaries, and an
// item names you when your first name is in it or in the name it is
// listed under.

const myFirstName = (m) => (m.my_name ? m.my_name.split(/\s+/)[0].toLowerCase() : null);

function namesMe(item, first) {
  if (!first) return false;
  const said = (text) => text.toLowerCase().split(/[^\p{L}\p{N}']+/u).includes(first);
  return said(item.text) || (item.owner != null && said(item.owner));
}

section("notes", "Notes",
  "Granola's AI summaries and the action items in them, and the notes you typed yourself.",
  (meetings, body) => {
    const grid = el("div", { class: "grid" });
    body.append(grid);
    typedNotes(grid, meetings);
    templateHistory(grid, meetings);
    actionItemsBySeries(body, meetings);
    myActionItems(body, meetings);
  });

function typedNotes(parent, meetings) {
  const rows = d3.rollups(meetings, (ms) => ({ meetings: ms.length, typed: ms.filter((m) => m.typed_note_chars > 0).length }),
    (m) => d3.timeMonth.floor(m.date)).map(([month, v]) => ({ month, ...v, share: v.typed / v.meetings }))
    .sort((a, b) => a.month - b.month);
  figure(parent, {
    title: "How often you typed notes",
    caption: "Share of each month's meetings where you typed notes of your own.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 220,
      x: { type: "band", label: null, tickFormat: monthTick },
      y: { tickFormat: "%", label: "meetings with notes", zero: true },
      marks: [
        gridY(),
        Plot.barY(rows, { x: "month", y: "share", fill: seriesColor(0), insetLeft: 2, insetRight: 2,
          tip: { channels: { meetings: "meetings", "with notes": "typed" }, format: { x: (d) => d3.timeFormat("%B %Y")(d), y: (v) => fmtPct(v) } } }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Month", value: (d) => d3.timeFormat("%Y-%m")(d.month) },
        { label: "Meetings", value: (d) => d.meetings, num: true },
        { label: "With typed notes", value: (d) => d.typed, num: true },
      ],
      rows: [...rows].reverse(),
    },
  });
}

function templateHistory(parent, meetings) {
  const summaries = meetings.flatMap((m) => m.summaries.map((s) => ({ m, template: s.template || "(none)" })));
  const top = d3.rollups(summaries, (v) => v.length, (s) => s.template).sort((a, b) => b[1] - a[1]).slice(0, 5).map(([t]) => t);
  const group = (t) => (top.includes(t) ? t : "Other templates");
  const groups = [...top, "Other templates"];
  const rows = d3.rollups(summaries, (v) => v.length, (s) => d3.timeMonth.floor(s.m.date), (s) => group(s.template))
    .flatMap(([month, gs]) => gs.map(([template, n]) => ({ month, template, n })));
  figure(parent, {
    title: "Which template wrote the AI notes",
    caption: "Summaries per month by Granola's template, which dates its changes to how it summarizes.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 240,
      x: { type: "band", label: null, tickFormat: monthTick },
      y: { label: "summaries" },
      color: { domain: groups, range: [...top.map((_, i) => seriesColor(i)), token("--other")], legend: true },
      marks: [
        gridY(),
        Plot.barY(rows, { x: "month", y: "n", fill: "template", order: groups, insetLeft: 2, insetRight: 2,
          stroke: token("--surface"), strokeWidth: 1, tip: { format: { x: (d) => d3.timeFormat("%B %Y")(d) } } }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Template", value: (d) => d[0] },
        { label: "Summaries", value: (d) => d[1].length, num: true },
        { label: "First", value: (d) => isoDay(d3.min(d[1], (s) => s.m.date)) },
        { label: "Last", value: (d) => isoDay(d3.max(d[1], (s) => s.m.date)) },
      ],
      rows: d3.groups(summaries, (s) => s.template).sort((a, b) => d3.min(a[1], (s) => s.m.date) - d3.min(b[1], (s) => s.m.date)),
    },
  });
}

function actionItemsBySeries(body, meetings) {
  const rows = d3.groups(meetings.filter((m) => m.series), (m) => m.series).map(([id, ms]) => {
    const items = ms.flatMap((m) => m.summaries.flatMap((s) => s.action_items.map((item) => ({ m, item }))));
    return {
      title: SERIES_TITLES.get(id),
      meetings: ms.length,
      items: items.length,
      mine: items.filter(({ m, item }) => namesMe(item, myFirstName(m))).length,
    };
  }).filter((r) => r.items > 0).sort((a, b) => b.items - a.items);
  const top = rows.slice(0, 15);
  const kinds = ["Naming you", "Others"];
  const stacked = top.flatMap((r) => [
    { title: r.title, kind: kinds[0], n: r.mine },
    { title: r.title, kind: kinds[1], n: r.items - r.mine },
  ]);
  figure(body, {
    title: "Action items per series",
    caption: "Items in the AI notes of each series' meetings, split by whether they name you, for the 15 series with the most.",
    plot: (width) => top.length ? Plot.plot({
      ...plotDefaults(width),
      height: 24 * top.length + 60,
      marginLeft: 180,
      x: { label: "action items", grid: true },
      y: { domain: top.map((r) => r.title), label: null },
      color: { domain: kinds, range: [seriesColor(1), seriesColor(0)], legend: true },
      marks: [
        Plot.barX(stacked, { x: "n", y: "title", fill: "kind", order: kinds, insetTop: 4, insetBottom: 4,
          stroke: token("--surface"), strokeWidth: 1, tip: true }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Series", value: (d) => d.title },
        { label: "Meetings", value: (d) => d.meetings, num: true },
        { label: "Action items", value: (d) => d.items, num: true },
        { label: "Naming you", value: (d) => d.mine, num: true },
      ],
      rows,
    },
  });
}

function myActionItems(body, meetings) {
  const rows = meetings.flatMap((m) => m.summaries.flatMap((s) => s.action_items
    .filter((item) => namesMe(item, myFirstName(m)))
    .map((item) => ({ m, item }))))
    .sort((a, b) => b.m.date - a.m.date);
  const card = figure(body, {
    title: "Action items naming you",
    caption: `${fmtInt(rows.length)} items, newest first.`,
  });
  if (rows.length) {
    card.append(el("div", { class: "table-wrap" }, renderTable({
      columns: [
        { label: "Day", value: (d) => isoDay(d.m.date) },
        { label: "Meeting", value: (d) => d.m.title },
        { label: "Item", value: (d) => d.item.text },
      ],
      rows,
    })));
  }
}
