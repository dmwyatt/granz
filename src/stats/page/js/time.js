section("time", "Time and load", "Where the hours go. A meeting's hours run from its first spoken word to its last.", (meetings, body) => {
  const transcribed = meetings.filter((m) => m.transcript);
  const weeks = d3.rollups(transcribed, (ms) => d3.sum(ms, (m) => m.spokenMinutes) / 60,
    (m) => d3.timeMonday.floor(m.date))
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
});
