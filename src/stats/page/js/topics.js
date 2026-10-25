// Topics. Mirrors src/stats/topics.rs. Groups, map positions and each
// meeting's similarity to its nearest neighbor and to the previous meeting
// of its series were computed over every embedded meeting in this file;
// the charts show the ones the filters select.

const TOPIC = new Map(DATA.topics.map.map((t) => [t.id, t]));
const CLUSTER_ORDER = DATA.topics.clusters.map((c) => c.id);
const clusterName = (id) => {
  const c = DATA.topics.clusters.find((c) => c.id === id);
  return c && c.label.length ? c.label.join(", ") : `group ${id + 1}`;
};

section("topics", "Topics",
  `What meetings are about, from the transcript embeddings: ${fmtInt(DATA.topics.embedded)} of ${fmtInt(DATA.topics.meetings)} meetings have one. Similarities are cosines between meetings; they rank meetings against each other and are not percentages.`,
  (meetings, body) => {
    const embedded = meetings.filter((m) => TOPIC.has(m.id)).map((m) => ({ m, t: TOPIC.get(m.id) }));
    if (!embedded.length) {
      body.append(el("p", { class: "note" }, "No meetings with an embedding in this selection. Run grans embed, then write the page again."));
      return;
    }
    topicMap(body, embedded);
    topicsOverTime(body, embedded);
    const grid = el("div", { class: "grid" });
    body.append(grid);
    rehashTable(grid, embedded);
    outlierTable(grid, embedded);
    twinsTable(body);
  });

function clusterColors() {
  const names = CLUSTER_ORDER.map(clusterName);
  return { domain: names, range: names.map((_, i) => seriesColor(i)) };
}

function topicMap(body, embedded) {
  figure(body, {
    title: "A map of your meetings",
    caption: "Each dot is a meeting, placed so that meetings about similar things sit close together (the first two principal components of their embeddings), colored by topic group. The axes have no units.",
    plot: (width) => Plot.plot({
      ...plotDefaults(width),
      height: Math.min(560, width * 0.6),
      inset: 12,
      x: { axis: null },
      y: { axis: null },
      color: { ...clusterColors(), legend: true },
      marks: [
        Plot.dot(embedded, { x: (d) => d.t.x, y: (d) => d.t.y, fill: (d) => clusterName(d.t.cluster), r: 3.5,
          stroke: token("--surface"), strokeWidth: 1,
          tip: { channels: { meeting: (d) => d.m.title, day: (d) => isoDay(d.m.date) }, format: { x: null, y: null } } }),
      ],
    }),
    table: {
      columns: [
        { label: "Meeting", value: (d) => d.m.title },
        { label: "Day", value: (d) => isoDay(d.m.date) },
        { label: "Topic group", value: (d) => clusterName(d.t.cluster) },
      ],
      rows: embedded.slice().sort((a, b) => b.m.date - a.m.date),
    },
  });
}

function topicsOverTime(body, embedded) {
  const rows = d3.rollups(embedded, (v) => v.length, (d) => d3.timeMonth.floor(d.m.date), (d) => clusterName(d.t.cluster))
    .flatMap(([month, groups]) => groups.map(([group, n]) => ({ month, group, n })));
  const colors = clusterColors();
  figure(body, {
    title: "Topics over time",
    caption: "Meetings per month in each topic group.",
    plot: (width) => Plot.plot({
      ...plotDefaults(width),
      height: 260,
      x: { type: "band", label: null, tickFormat: monthTick },
      y: { label: "meetings" },
      color: { ...colors, legend: true },
      marks: [
        gridY(),
        Plot.barY(rows, { x: "month", y: "n", fill: "group", order: colors.domain, insetLeft: 2, insetRight: 2,
          stroke: token("--surface"), strokeWidth: 1, tip: { format: { x: (d) => d3.timeFormat("%B %Y")(d) } } }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }),
    table: {
      columns: [
        { label: "Month", value: (d) => d3.timeFormat("%Y-%m")(d.month) },
        { label: "Topic group", value: (d) => d.group },
        { label: "Meetings", value: (d) => d.n, num: true },
      ],
      rows: rows.slice().sort((a, b) => b.month - a.month || b.n - a.n),
    },
  });
}

function rehashTable(parent, embedded) {
  const rows = d3.groups(embedded.filter((d) => d.m.series && d.t.previous_in_series != null), (d) => d.m.series)
    .map(([id, ds]) => {
      ds.sort((a, b) => a.m.date - b.m.date);
      return {
        title: SERIES_TITLES.get(id),
        pairs: ds.length,
        typical: d3.median(ds, (d) => d.t.previous_in_series),
        latest: ds[ds.length - 1].t.previous_in_series,
      };
    })
    .filter((r) => r.pairs >= 2)
    .sort((a, b) => b.typical - a.typical);
  const card = figure(parent, {
    title: "Recurring meetings that repeat themselves",
    caption: "How alike each meeting of a series is to the one before it: typically, and the latest. High means the same conversation again.",
  });
  card.append(el("div", { class: "table-wrap" }, renderTable({
    columns: [
      { label: "Series", value: (d) => d.title },
      { label: "Typical", value: (d) => d.typical.toFixed(2), num: true },
      { label: "Latest", value: (d) => d.latest.toFixed(2), num: true },
      { label: "Pairs", value: (d) => d.pairs, num: true },
    ],
    rows,
  })));
}

function outlierTable(parent, embedded) {
  const rows = embedded.slice().sort((a, b) => a.t.nearest - b.t.nearest).slice(0, 15);
  const card = figure(parent, {
    title: "Meetings unlike any other",
    caption: "The meetings whose most similar other meeting is least similar.",
  });
  card.append(el("div", { class: "table-wrap" }, renderTable({
    columns: [
      { label: "Meeting", value: (d) => d.m.title },
      { label: "Day", value: (d) => isoDay(d.m.date) },
      { label: "Nearest", value: (d) => d.t.nearest.toFixed(2), num: true },
    ],
    rows,
  })));
}

function twinsTable(body) {
  const card = figure(body, {
    title: "Different series, nearly the same content",
    caption: "Series compared by the average of their meetings, for series with at least 3 embedded meetings. Over every meeting in this file.",
  });
  if (!DATA.topics.twins.length) {
    card.append(el("div", { class: "empty" }, "Not enough recurring meetings with embeddings."));
    return;
  }
  card.append(el("div", { class: "table-wrap" }, renderTable({
    columns: [
      { label: "Series", value: (d) => d.a_title },
      { label: "Series", value: (d) => d.b_title },
      { label: "Similarity", value: (d) => d.similarity.toFixed(2), num: true },
    ],
    rows: DATA.topics.twins,
  })));
}
