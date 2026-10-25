// Recurring series. Mirrors src/stats/series.rs: a series is the calendar's
// recurring event id, titled by its latest meeting, and punctuality uses a
// meeting's first session of talk against its slot.

// The series with the most hours across all the data keep the same color
// whatever the filters select; the rest share "Other recurring".
const TOP_SERIES = d3.rollups(MEETINGS.filter((m) => m.series), (ms) => d3.sum(ms, (m) => m.spokenMinutes), (m) => m.series)
  .sort((a, b) => b[1] - a[1])
  .slice(0, 6)
  .map(([id]) => id);
const OTHER_RECURRING = "Other recurring";
const ONE_OFF = "One-off";

function seriesGroup(m) {
  if (!m.series) return ONE_OFF;
  return TOP_SERIES.includes(m.series) ? SERIES_TITLES.get(m.series) : OTHER_RECURRING;
}

function punctuality(m) {
  if (!m.scheduled || !m.sessions.length) return null;
  const first = m.sessions[0];
  return {
    startDelay: (first.start - new Date(m.scheduled.start)) / 60000,
    overrun: (first.end - new Date(m.scheduled.end)) / 60000,
  };
}

function seriesRollups(meetings) {
  return d3.groups(meetings.filter((m) => m.series), (m) => m.series).map(([id, ms]) => {
    ms.sort((a, b) => a.date - b.date);
    const gaps = d3.pairs(ms, (a, b) => (b.date - a.date) / DAY);
    const timings = ms.map(punctuality).filter(Boolean);
    return {
      id,
      title: SERIES_TITLES.get(id),
      meetings: ms.length,
      hours: d3.sum(ms, (m) => m.spokenMinutes) / 60,
      first: ms[0].date,
      last: ms[ms.length - 1].date,
      everyDays: gaps.length ? d3.median(gaps) : null,
      invitedFirst: ms[0].invited,
      invitedLatest: ms[ms.length - 1].invited,
      startDelay: timings.length ? d3.median(timings, (t) => t.startDelay) : null,
      overrun: timings.length ? d3.median(timings, (t) => t.overrun) : null,
      myShare: share(d3.sum(ms, (m) => m.myChars), d3.sum(ms, (m) => m.othersChars)),
    };
  }).sort((a, b) => b.hours - a.hours);
}

const fmtSigned = (m) => (m == null ? "–" : `${m >= 0 ? "+" : ""}${Math.round(m)}`);

section("series", "Recurring meetings",
  "A series is the calendar's recurring event. Editing a recurring meeting from some date on can start a new series, so one meeting can appear as several.",
  (meetings, body) => {
    hoursBySeries(body, meetings);
    seriesTable(body, meetings);
    const grid = el("div", { class: "grid" });
    body.append(grid);
    const timings = meetings.map(punctuality).filter(Boolean);
    punctualityHistogram(grid, timings, {
      title: "When the talk starts",
      caption: "Minutes between the scheduled start and the first word. Positive is late.",
      value: (t) => t.startDelay, domain: [-10, 20], step: 1,
    });
    punctualityHistogram(grid, timings, {
      title: "When the talk ends",
      caption: "Minutes between the scheduled end and the end of the first stretch of talk. Positive ran over.",
      value: (t) => t.overrun, domain: [-60, 60], step: 5,
    });
  });

function hoursBySeries(body, meetings) {
  const groups = [...TOP_SERIES.map((id) => SERIES_TITLES.get(id)), OTHER_RECURRING, ONE_OFF];
  const colors = [...TOP_SERIES.map((_, i) => seriesColor(i)), token("--other"), token("--grid")];
  const rows = d3.rollups(meetings.flatMap((m) => m.sessions.map((s) => ({ s, group: seriesGroup(m) }))),
    (xs) => d3.sum(xs, (x) => hoursOf(x.s)), (x) => d3.timeMonday.floor(x.s.start), (x) => x.group)
    .flatMap(([week, byGroup]) => byGroup.map(([group, hours]) => ({ week, group, hours })));

  figure(body, {
    title: "Weekly hours by series",
    caption: "The six series with the most hours overall have their own color; the rest are grouped.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 280,
      x: { type: "time", label: null },
      y: { label: "hours" },
      color: { domain: groups, range: colors, legend: true },
      marks: [
        gridY(),
        Plot.rectY(rows, Plot.stackY({
          x1: "week", x2: (d) => d3.timeMonday.offset(d.week, 1), y: "hours", fill: "group",
          order: groups, insetLeft: 1, insetRight: 1, stroke: token("--surface"), strokeWidth: 1,
          tip: { format: { x1: (d) => `week of ${isoDay(d)}`, x2: null, y: (h) => `${h.toFixed(1)} h` } },
        })),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Week of", value: (d) => isoDay(d.week) },
        { label: "Series", value: (d) => d.group },
        { label: "Hours", value: (d) => d.hours.toFixed(1), num: true },
      ],
      rows: [...rows].sort((a, b) => b.week - a.week || b.hours - a.hours),
    },
  });
}

function seriesTable(body, meetings) {
  const rollups = seriesRollups(meetings);
  const card = figure(body, {
    title: "Every series",
    caption: "Most hours first. Every: median days between meetings. Invited: first meeting, then latest. Start and overrun: median minutes late and over.",
  });
  if (!rollups.length) {
    card.append(el("div", { class: "empty" }, "No recurring meetings in this selection."));
    return;
  }
  card.append(el("div", { class: "table-wrap" }, renderTable({
    columns: [
      { label: "Series", value: (d) => d.title },
      { label: "Meetings", value: (d) => d.meetings, num: true },
      { label: "Hours", value: (d) => d.hours.toFixed(1), num: true },
      { label: "First", value: (d) => isoDay(d.first) },
      { label: "Last", value: (d) => isoDay(d.last) },
      { label: "Every", value: (d) => (d.everyDays == null ? "–" : `${Math.round(d.everyDays)}d`), num: true },
      { label: "Invited", value: (d) => `${d.invitedFirst} → ${d.invitedLatest}`, num: true },
      { label: "Start", value: (d) => fmtSigned(d.startDelay), num: true },
      { label: "Overrun", value: (d) => fmtSigned(d.overrun), num: true },
      { label: "You", value: (d) => fmtPct(d.myShare), num: true },
    ],
    rows: rollups,
  })));
}

function punctualityHistogram(parent, timings, { title, caption, value, domain, step }) {
  const values = timings.map(value);
  const inside = values.filter((v) => v >= domain[0] && v < domain[1]);
  const outside = values.length - inside.length;
  const bins = d3.bin().domain(domain).thresholds(d3.range(domain[0], domain[1] + step, step))(inside)
    .map((b) => ({ x0: b.x0, x1: b.x1, count: b.length }));

  figure(parent, {
    title,
    caption: outside ? `${caption} ${fmtInt(outside)} meetings fall outside ${domain[0]} to +${domain[1]} and are not drawn.` : caption,
    plot: (width) => values.length ? Plot.plot({
      ...plotDefaults(width),
      height: 220,
      x: { label: "minutes", domain },
      y: { label: "meetings" },
      marks: [
        gridY(),
        Plot.rectY(bins, { x1: "x0", x2: "x1", y: "count", fill: seriesColor(0), inset: 1,
          tip: { format: { x1: (v) => `${fmtSigned(v)} min`, x2: (v) => `${fmtSigned(v)} min` } } }),
        Plot.ruleX([0], { stroke: token("--ink-2") }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "From (min)", value: (d) => fmtSigned(d.x0), num: true },
        { label: "To (min)", value: (d) => fmtSigned(d.x1), num: true },
        { label: "Meetings", value: (d) => d.count, num: true },
      ],
      rows: bins,
    },
  });
}
