// Time and load. Mirrors src/stats/time.rs: a meeting occupies its sessions
// of talk, days and hours are local, free time is measured from 09:00 to
// 17:00, and meetings at most 10 minutes apart are back to back.

const WORKDAY = [9, 17];
const BACK_TO_BACK_MS = 10 * 60000;
const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const SIZE_BUCKETS = ["none", "1", "2", "3-5", "6-10", "11+"];

const timeOfDay = (d) => d.getHours() + d.getMinutes() / 60;
const weekdayIndex = (d) => (d.getDay() + 6) % 7;

function sizeBucket(invited) {
  if (invited <= 2) return SIZE_BUCKETS[invited];
  if (invited <= 5) return "3-5";
  if (invited <= 10) return "6-10";
  return "11+";
}

// One entry per local day with talk: { day, sessions (sorted), hours,
// first, last, longestFree (minutes), streak }.
function dayLoads(meetings) {
  const byDay = d3.group(meetings.flatMap((m) => m.sessions), (s) => isoDay(s.start));
  return [...byDay].map(([day, sessions]) => {
    sessions.sort((a, b) => a.start - b.start);
    const date = new Date(day + "T00:00:00");
    return {
      day: date,
      sessions,
      hours: d3.sum(sessions, hoursOf),
      first: d3.min(sessions, (s) => timeOfDay(s.start)),
      last: d3.max(sessions, (s) => timeOfDay(s.end)),
      longestFree: longestFree(date, sessions),
      streak: longestStreak(sessions),
    };
  }).sort((a, b) => a.day - b.day);
}

function longestFree(date, sessions) {
  const at = (h) => new Date(date.getFullYear(), date.getMonth(), date.getDate(), h);
  const open = at(WORKDAY[0]);
  const close = at(WORKDAY[1]);
  let cursor = open;
  let longest = 0;
  for (const s of sessions) {
    const start = s.start < open ? open : s.start;
    const end = s.end > close ? close : s.end;
    if (start > cursor) longest = Math.max(longest, Math.min(start, close) - cursor);
    if (end > cursor) cursor = end;
  }
  longest = Math.max(longest, close - Math.min(cursor, close));
  return Math.floor(longest / 60000);
}

function longestStreak(sessions) {
  let longest = 0;
  let current = 0;
  let previousEnd = null;
  for (const s of sessions) {
    current = previousEnd && s.start - previousEnd <= BACK_TO_BACK_MS ? current + 1 : 1;
    longest = Math.max(longest, current);
    previousEnd = previousEnd && previousEnd > s.end ? previousEnd : s.end;
  }
  return longest;
}

// Hours of talk in each weekday and hour, sessions spread across the hours
// they cover.
function weekdayHourGrid(meetings) {
  const grid = new Map();
  for (const s of meetings.flatMap((m) => m.sessions)) {
    let cursor = s.start;
    while (cursor < s.end) {
      const nextHour = new Date(cursor);
      nextHour.setHours(cursor.getHours() + 1, 0, 0, 0);
      const sliceEnd = nextHour < s.end ? nextHour : s.end;
      const key = `${weekdayIndex(cursor)}:${cursor.getHours()}`;
      grid.set(key, (grid.get(key) || 0) + (sliceEnd - cursor) / 3600000);
      cursor = sliceEnd;
    }
  }
  return [...grid].map(([key, hours]) => {
    const [weekday, hour] = key.split(":").map(Number);
    return { weekday: WEEKDAYS[weekday], hour, hours };
  });
}

const fmtClock = (h) => `${String(Math.floor(h)).padStart(2, "0")}:${String(Math.round((h % 1) * 60) % 60).padStart(2, "0")}`;
const fmtMinutes = (m) => `${Math.floor(m / 60)}h${String(m % 60).padStart(2, "0")}`;

function sequentialScale() {
  return { type: "linear", range: [token("--seq-low"), token("--seq-high")], interpolate: "rgb" };
}

section("time", "Time and load",
  "Where the hours go. A meeting's hours are its spoken time: first word to last, leaving out silences over 30 minutes.",
  (meetings, body) => {
    const days = dayLoads(meetings);
    weeklyHours(body, meetings);
    calendarHeatmap(body, days);
    const grid = el("div", { class: "grid" });
    body.append(grid);
    weekdayHours(grid, meetings);
    workdayEdges(grid, days);
    const frag = el("div", { class: "grid" });
    body.append(frag);
    freeTime(frag, days);
    backToBack(frag, days);
    sizeMix(body, meetings);
  });

function weeklyHours(body, meetings) {
  const sessions = meetings.flatMap((m) => m.sessions);
  const weeks = d3.rollups(sessions, (ss) => d3.sum(ss, hoursOf), (s) => d3.timeMonday.floor(s.start))
    .map(([week, hours]) => ({ week, hours }))
    .sort((a, b) => a.week - b.week);

  figure(body, {
    title: "Meeting hours per week",
    caption: "Weeks start on Monday. Hover a bar for its total.",
    plot: (width) => weeks.length ? Plot.plot({
      ...plotDefaults(width),
      height: 240,
      x: { type: "time", label: null },
      y: { label: "hours", grid: false },
      marks: [
        gridY(),
        Plot.rectY(weeks, {
          x1: "week",
          x2: (d) => d3.timeMonday.offset(d.week, 1),
          y: "hours",
          fill: seriesColor(0),
          inset: 1,
          tip: { format: { x1: (d) => `week of ${isoDay(d)}`, x2: null, y: (h) => `${h.toFixed(1)} h` } },
        }),
        Plot.ruleY([0], { stroke: token("--axis") }),
        ...eraMarks(d3.extent(weeks, (d) => d.week)),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Week of", value: (d) => isoDay(d.week) },
        { label: "Hours", value: (d) => d.hours.toFixed(1), num: true },
      ],
      rows: [...weeks].reverse(),
    },
  });
}

function calendarHeatmap(parent, days) {
  const cells = days.map((d) => ({ ...d, week: d3.timeMonday.floor(d.day), weekday: WEEKDAYS[weekdayIndex(d.day)] }));
  const weeks = [...new Set(cells.map((c) => +c.week))].sort((a, b) => a - b).map((w) => new Date(w));
  // Label the first week of each month, skipping a month whose label would
  // crowd the previous one.
  const monthStarts = new Set();
  let lastLabel = -Infinity;
  weeks.forEach((w, i) => {
    if ((i === 0 || w.getMonth() !== weeks[i - 1].getMonth()) && i - lastLabel >= 3) {
      monthStarts.add(+w);
      lastLabel = i;
    }
  });

  figure(parent, {
    title: "Every day, shaded by meeting hours",
    caption: "Columns are weeks; empty cells had no meetings.",
    plot: (width) => cells.length ? Plot.plot({
      ...plotDefaults(width),
      height: 200,
      marginLeft: 40,
      x: {
        type: "band",
        domain: weeks,
        label: null,
        tickFormat: (w) => (monthStarts.has(+w) ? d3.timeFormat(w.getMonth() === 0 ? "%Y" : "%b")(w) : ""),
        tickSize: 0,
      },
      y: { domain: WEEKDAYS, label: null, tickSize: 0 },
      color: { ...sequentialScale(), label: "hours", legend: true },
      marks: [
        Plot.cell(cells, {
          x: "week",
          y: "weekday",
          fill: "hours",
          inset: 1,
          rx: 2,
          tip: { channels: { day: (d) => isoDay(d.day), meetings: (d) => d.sessions.length },
            format: { x: null, y: null, fill: (h) => `${h.toFixed(1)} h` } },
        }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Day", value: (d) => isoDay(d.day) },
        { label: "Meetings", value: (d) => d.sessions.length, num: true },
        { label: "Hours", value: (d) => d.hours.toFixed(1), num: true },
      ],
      rows: [...days].reverse(),
    },
  });
}

function weekdayHours(parent, meetings) {
  const cells = weekdayHourGrid(meetings);
  const hours = cells.filter((c) => c.hours >= 0.05).map((c) => c.hour);
  const domain = hours.length ? d3.range(d3.min(hours), d3.max(hours) + 1) : [];

  figure(parent, {
    title: "When in the week meetings happen",
    caption: "Hours of talk in each hour of the day (columns) for each weekday, summed over the selection.",
    plot: (width) => domain.length ? Plot.plot({
      ...plotDefaults(width),
      height: 220,
      marginLeft: 40,
      x: { type: "band", domain, label: null, tickFormat: (h) => String(h).padStart(2, "0") },
      y: { domain: WEEKDAYS, label: null, tickSize: 0 },
      color: { ...sequentialScale(), label: "hours", legend: true },
      marks: [
        Plot.cell(cells.filter((c) => domain.includes(c.hour)), {
          x: "hour",
          y: "weekday",
          fill: "hours",
          inset: 1,
          rx: 2,
          tip: { format: { x: (h) => `${String(h).padStart(2, "0")}:00`, fill: (h) => `${h.toFixed(1)} h` } },
        }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Weekday", value: (d) => d.weekday },
        { label: "Hour", value: (d) => `${String(d.hour).padStart(2, "0")}:00` },
        { label: "Hours", value: (d) => d.hours.toFixed(1), num: true },
      ],
      rows: [...cells].sort((a, b) => WEEKDAYS.indexOf(a.weekday) - WEEKDAYS.indexOf(b.weekday) || a.hour - b.hour),
    },
  });
}

function workdayEdges(body, days) {
  const points = days.flatMap((d) => [
    { day: d.day, edge: "First word", time: d.first },
    { day: d.day, edge: "Last word", time: d.last },
  ]);
  const colors = { domain: ["First word", "Last word"], range: [seriesColor(0), seriesColor(1)] };

  figure(body, {
    title: "Workday edges",
    caption: "Each day's first and last spoken word in a meeting. Lines are the median of the surrounding 15 meeting days.",
    plot: (width) => points.length ? Plot.plot({
      ...plotDefaults(width),
      height: 260,
      x: { type: "time", label: null },
      y: { label: "time of day", reverse: true, tickFormat: fmtClock, domain: [d3.min(days, (d) => d.first) - 0.5, d3.max(days, (d) => d.last) + 0.5] },
      color: { ...colors, legend: true },
      marks: [
        gridY(),
        Plot.dot(points, { x: "day", y: "time", fill: "edge", r: 2, fillOpacity: 0.35,
          tip: { format: { x: (d) => isoDay(d), y: fmtClock } } }),
        Plot.lineY(points, Plot.windowY({ k: 15, reduce: "median" },
          { x: "day", y: "time", stroke: "edge", strokeWidth: 2 })),
        ...eraMarks(d3.extent(days, (d) => d.day)),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Day", value: (d) => isoDay(d.day) },
        { label: "First word", value: (d) => fmtClock(d.first), num: true },
        { label: "Last word", value: (d) => fmtClock(d.last), num: true },
      ],
      rows: [...days].reverse(),
    },
  });
}

function weeklyFromDays(days, reduce) {
  return d3.rollups(days, reduce, (d) => d3.timeMonday.floor(d.day))
    .map(([week, value]) => ({ week, value }))
    .sort((a, b) => a.week - b.week);
}

function freeTime(parent, days) {
  const weeks = weeklyFromDays(days, (ds) => d3.median(ds, (d) => d.longestFree));
  figure(parent, {
    title: "Longest free stretch",
    caption: "The longest gap between meetings from 09:00 to 17:00 on a meeting day; weekly median.",
    plot: (width) => weeks.length ? Plot.plot({
      ...plotDefaults(width),
      height: 220,
      x: { type: "time", label: null },
      y: { label: "hours", domain: [0, (WORKDAY[1] - WORKDAY[0])] },
      marks: [
        gridY(),
        Plot.lineY(weeks, { x: "week", y: (d) => d.value / 60, stroke: seriesColor(0), strokeWidth: 2,
          tip: { format: { x: (d) => `week of ${isoDay(d)}`, y: (h) => fmtMinutes(Math.round(h * 60)) } } }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Week of", value: (d) => isoDay(d.week) },
        { label: "Median longest free stretch", value: (d) => fmtMinutes(Math.round(d.value)), num: true },
      ],
      rows: [...weeks].reverse(),
    },
  });
}

function backToBack(parent, days) {
  const weeks = weeklyFromDays(days, (ds) => d3.max(ds, (d) => d.streak));
  figure(parent, {
    title: "Longest back-to-back run",
    caption: "The most meetings in a row, at most 10 minutes apart, on any day of the week.",
    plot: (width) => weeks.length ? Plot.plot({
      ...plotDefaults(width),
      height: 220,
      x: { type: "time", label: null },
      y: { label: "meetings in a row", tickFormat: "d", ticks: d3.max(weeks, (d) => d.value) },
      marks: [
        gridY(),
        Plot.rectY(weeks, { x1: "week", x2: (d) => d3.timeMonday.offset(d.week, 1), y: "value",
          fill: seriesColor(0), inset: 1,
          tip: { format: { x1: (d) => `week of ${isoDay(d)}`, x2: null } } }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Week of", value: (d) => isoDay(d.week) },
        { label: "Longest run", value: (d) => d.value, num: true },
      ],
      rows: [...weeks].reverse(),
    },
  });
}

function sizeMix(body, meetings) {
  const months = d3.rollups(meetings, (ms) => d3.rollup(ms, (v) => v.length, (m) => sizeBucket(m.invited)),
    (m) => d3.timeMonth.floor(m.date))
    .sort((a, b) => a[0] - b[0]);
  const rows = months.flatMap(([month, counts]) =>
    SIZE_BUCKETS.map((bucket) => ({ month, bucket, count: counts.get(bucket) || 0 })));
  const ramp = d3.quantize(d3.interpolateRgb(token("--seq-low"), token("--seq-high")), SIZE_BUCKETS.length + 1).slice(1);

  figure(body, {
    title: "Meeting sizes over time",
    caption: "Meetings per month by invitees who did not decline, not counting you.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 260,
      x: { type: "band", label: null, tickFormat: (d) => d3.timeFormat(d.getMonth() === 0 ? "%b %Y" : "%b")(d) },
      y: { label: "meetings" },
      color: { domain: SIZE_BUCKETS, range: ramp, legend: true, label: "invitees" },
      marks: [
        gridY(),
        Plot.barY(rows, { x: "month", y: "count", fill: "bucket", order: SIZE_BUCKETS, insetLeft: 2, insetRight: 2,
          stroke: token("--surface"), strokeWidth: 1,
          tip: { format: { x: (d) => d3.timeFormat("%B %Y")(d) } } }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Month", value: (d) => d3.timeFormat("%Y-%m")(d[0]) },
        ...SIZE_BUCKETS.map((b) => ({ label: b, value: (d) => d[1].get(b) || 0, num: true })),
      ],
      rows: [...months].reverse(),
    },
  });
}
