// People. Mirrors src/stats/people.rs: a person is an invitee's email,
// meetings with them are ones they were invited to and did not decline, a
// one-on-one has exactly one such invitee, and pairs only count meetings
// of up to 10 invitees.

const MAX_PAIR_MEETING_SIZE = 10;

const attending = (m) => m.invitees.filter((i) => i.response !== "declined");
const domainOf = (email) => email.slice(email.lastIndexOf("@") + 1);

// The user's domain: the most common domain of their own address.
const MY_DOMAIN = (() => {
  const counts = d3.rollups(MEETINGS.filter((m) => m.my_email), (v) => v.length, (m) => domainOf(m.my_email));
  return counts.length ? counts.sort((a, b) => b[1] - a[1])[0][0] : null;
})();

function personRollups(meetings) {
  const byPerson = new Map();
  for (const m of meetings) {
    for (const i of m.invitees) {
      const p = byPerson.get(i.email) || { email: i.email, meetings: [], responses: [] };
      p.responses.push(i.response);
      if (i.response !== "declined") p.meetings.push(m);
      byPerson.set(i.email, p);
    }
  }
  return [...byPerson.values()].filter((p) => p.meetings.length).map((p) => {
    const ones = p.meetings.filter((m) => m.invited === 1).sort((a, b) => a.date - b.date);
    const answered = p.responses.filter((r) => r != null);
    return {
      email: p.email,
      name: personName(p.email),
      meetings: p.meetings.length,
      hours: d3.sum(p.meetings, (m) => m.spokenMinutes) / 60,
      first: d3.min(p.meetings, (m) => m.date),
      last: d3.max(p.meetings, (m) => m.date),
      oneOnOnes: ones.length,
      oneOnOneEvery: ones.length > 1 ? d3.median(d3.pairs(ones, (a, b) => (b.date - a.date) / DAY)) : null,
      lastOneOnOne: ones.length ? ones[ones.length - 1].date : null,
      myShare: share(d3.sum(ones, (m) => m.myChars), d3.sum(ones, (m) => m.othersChars)),
      acceptRate: answered.length ? answered.filter((r) => r === "accepted").length / answered.length : null,
    };
  }).sort((a, b) => b.meetings - a.meetings);
}

section("people", "People",
  "Who you meet with. An invitation does not say who came, so a meeting with someone means they were invited and did not decline.",
  (meetings, body) => {
    const people = personRollups(meetings);
    const grid = el("div", { class: "grid" });
    body.append(grid);
    oneOnOneShare(grid, people);
    organizerHours(grid, meetings);
    network(body, meetings);
    relationshipTimelines(body, meetings, people);
    internalExternal(body, meetings);
    peopleTable(body, people, meetings);
  });

function oneOnOneShare(parent, people) {
  const rows = people.filter((p) => p.oneOnOnes > 0 && p.myShare != null)
    .sort((a, b) => b.oneOnOnes - a.oneOnOnes).slice(0, 15);
  figure(parent, {
    title: "Who talks more in your one-on-ones",
    caption: "Your share of the talk with each person, for the 15 you have the most one-on-ones with. The rule marks an even split.",
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 24 * rows.length + 40,
      marginLeft: 130,
      x: { domain: [0, 1], label: "your share", tickFormat: "%", grid: true },
      y: { domain: rows.map((r) => r.name), label: null },
      marks: [
        Plot.barX(rows, { x: "myShare", y: "name", fill: seriesColor(0), insetTop: 4, insetBottom: 4,
          tip: { channels: { "one-on-ones": "oneOnOnes" }, format: { x: (v) => fmtPct(v) } } }),
        Plot.ruleX([0.5], { stroke: token("--ink-2") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Person", value: (d) => d.name },
        { label: "One-on-ones", value: (d) => d.oneOnOnes, num: true },
        { label: "Your share", value: (d) => fmtPct(d.myShare), num: true },
      ],
      rows,
    },
  });
}

function organizerHours(parent, meetings) {
  const rows = d3.rollups(meetings.filter((m) => m.organizer),
    (ms) => ({ hours: d3.sum(ms, (m) => m.spokenMinutes) / 60, meetings: ms.length, me: ms.some((m) => m.organized_by_me) }),
    (m) => m.organizer)
    .map(([email, r]) => ({ email, name: r.me ? "You" : personName(email), ...r }))
    .sort((a, b) => b.hours - a.hours);
  const top = rows.slice(0, 12);
  const mine = d3.sum(rows.filter((r) => r.me), (r) => r.meetings);
  const total = d3.sum(rows, (r) => r.meetings);

  figure(parent, {
    title: "Who fills your calendar",
    caption: `Hours of meetings by organizer. You organized ${fmtPct(total ? mine / total : null)} of the meetings with a known organizer.`,
    plot: (width) => top.length ? Plot.plot({
      ...plotDefaults(width),
      height: 24 * top.length + 40,
      marginLeft: 130,
      x: { label: "hours", grid: true },
      y: { domain: top.map((r) => r.name), label: null },
      marks: [
        Plot.barX(top, { x: "hours", y: "name", fill: (d) => (d.me ? seriesColor(1) : seriesColor(0)),
          insetTop: 4, insetBottom: 4,
          tip: { channels: { meetings: "meetings" }, format: { fill: null, x: (h) => `${h.toFixed(1)} h` } } }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Organizer", value: (d) => d.name },
        { label: "Meetings", value: (d) => d.meetings, num: true },
        { label: "Hours", value: (d) => d.hours.toFixed(1), num: true },
      ],
      rows,
    },
  });
}

// Communities by label propagation: each person takes the label carrying
// the most weight among their neighbors, visiting people in a fixed order
// so the result is stable.
function communities(nodes, links) {
  const label = new Map(nodes.map((n) => [n.id, n.id]));
  const neighbors = new Map(nodes.map((n) => [n.id, []]));
  for (const l of links) {
    neighbors.get(l.source).push([l.target, l.weight]);
    neighbors.get(l.target).push([l.source, l.weight]);
  }
  for (let round = 0; round < 20; round++) {
    let changed = false;
    for (const n of nodes) {
      const weights = new Map();
      for (const [other, w] of neighbors.get(n.id)) {
        const l = label.get(other);
        weights.set(l, (weights.get(l) || 0) + w);
      }
      if (!weights.size) continue;
      const best = [...weights].sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1))[0][0];
      if (best !== label.get(n.id)) {
        label.set(n.id, best);
        changed = true;
      }
    }
    if (!changed) break;
  }
  return label;
}

function network(body, meetings) {
  const pairCounts = new Map();
  const meetingsOf = new Map();
  for (const m of meetings) {
    const emails = [...new Set(attending(m).map((i) => i.email))].sort();
    for (const e of emails) meetingsOf.set(e, (meetingsOf.get(e) || 0) + 1);
    if (emails.length > MAX_PAIR_MEETING_SIZE) continue;
    for (let a = 0; a < emails.length; a++) {
      for (let b = a + 1; b < emails.length; b++) {
        const key = `${emails[a]}\n${emails[b]}`;
        pairCounts.set(key, (pairCounts.get(key) || 0) + 1);
      }
    }
  }
  const nodes = [...meetingsOf].sort((a, b) => b[1] - a[1]).slice(0, 40)
    .map(([id, count]) => ({ id, count, name: personName(id) }));
  const ids = new Set(nodes.map((n) => n.id));
  const links = [...pairCounts]
    .map(([key, weight]) => { const [source, target] = key.split("\n"); return { source, target, weight }; })
    .filter((l) => ids.has(l.source) && ids.has(l.target) && l.weight >= 2);

  const label = communities(nodes, links);
  const sizes = d3.rollups(nodes, (v) => v.length, (n) => label.get(n.id)).sort((a, b) => b[1] - a[1]);
  const colored = new Map(sizes.filter(([, n]) => n > 1).slice(0, 6).map(([l], i) => [l, seriesColor(i)]));

  figure(body, {
    title: "Who is invited together",
    caption: "The 40 people you share the most meetings with. Lines join people invited together to at least two meetings of up to 10 invitees, thicker for more; colors are groups found from those links, which tend to be teams.",
    plot: (width) => nodes.length ? drawNetwork(width, nodes, links, (n) => colored.get(label.get(n.id)) || token("--other")) : null,
    table: {
      columns: [
        { label: "Person", value: (d) => d.a },
        { label: "Person", value: (d) => d.b },
        { label: "Meetings together", value: (d) => d.weight, num: true },
      ],
      rows: links.map((l) => ({ a: personName(l.source), b: personName(l.target), weight: l.weight }))
        .sort((x, y) => y.weight - x.weight),
    },
  });
}

function drawNetwork(width, nodes, links, color) {
  const height = Math.min(560, Math.max(380, width * 0.6));
  const strongest = d3.max(links, (l) => l.weight) || 1;
  const sim = d3.forceSimulation(nodes.map((n) => ({ ...n })))
    .force("link", d3.forceLink(links.map((l) => ({ ...l }))).id((n) => n.id)
      .distance(70).strength((l) => 0.05 + 0.3 * (l.weight / strongest)))
    .force("charge", d3.forceManyBody().strength(-260).distanceMax(500))
    .force("x", d3.forceX(0).strength(0.04))
    .force("y", d3.forceY(0).strength(0.06))
    .force("collide", d3.forceCollide(16))
    .stop();
  sim.tick(400);
  const placed = sim.nodes();
  const placedLinks = sim.force("link").links();
  const radius = d3.scaleSqrt().domain([0, d3.max(placed, (n) => n.count)]).range([4, 14]);
  const stroke = d3.scaleLinear().domain([2, strongest]).range([0.5, 4]);
  // Fit the finished layout to the frame, leaving room for labels.
  const fitX = d3.scaleLinear().domain(d3.extent(placed, (n) => n.x)).range([24, width - 120]);
  const fitY = d3.scaleLinear().domain(d3.extent(placed, (n) => n.y)).range([24, height - 24]);
  const clampX = (x) => fitX(x);
  const clampY = (y) => fitY(y);

  const svg = d3.create("svg").attr("width", width).attr("height", height).attr("viewBox", [0, 0, width, height]);
  svg.append("g").attr("stroke", token("--axis")).selectAll("line").data(placedLinks).join("line")
    .attr("x1", (l) => clampX(l.source.x)).attr("y1", (l) => clampY(l.source.y))
    .attr("x2", (l) => clampX(l.target.x)).attr("y2", (l) => clampY(l.target.y))
    .attr("stroke-width", (l) => stroke(l.weight));
  const node = svg.append("g").selectAll("g").data(placed).join("g")
    .attr("transform", (n) => `translate(${clampX(n.x)},${clampY(n.y)})`);
  node.append("circle").attr("r", (n) => radius(n.count)).attr("fill", color)
    .attr("stroke", token("--surface")).attr("stroke-width", 2);
  node.append("title").text((n) => `${n.name}: ${n.count} meetings`);
  const labelled = new Set(placed.slice().sort((a, b) => b.count - a.count).slice(0, 12).map((n) => n.id));
  node.filter((n) => labelled.has(n.id)).append("text")
    .attr("x", (n) => radius(n.count) + 4).attr("dy", "0.35em")
    .attr("fill", token("--ink-2")).attr("font-size", 11).text((n) => n.name);
  return svg.node();
}

function relationshipTimelines(body, meetings, people) {
  const top = people.slice().sort((a, b) => b.hours - a.hours).slice(0, 16);
  const months = d3.timeMonth.range(d3.timeMonth.floor(d3.min(meetings, (m) => m.date) || GENERATED),
    d3.timeMonth.offset(d3.timeMonth.floor(d3.max(meetings, (m) => m.date) || GENERATED), 1));
  const rows = top.flatMap((p) => {
    const mine = meetings.filter((m) => attending(m).some((i) => i.email === p.email));
    const byMonth = d3.rollup(mine, (ms) => d3.sum(ms, (m) => m.spokenMinutes) / 60, (m) => +d3.timeMonth.floor(m.date));
    return months.map((month) => ({ person: p.name, month, hours: byMonth.get(+month) || 0 }));
  });

  figure(body, {
    title: "Time together, month by month",
    caption: "Hours of meetings each person was invited to, for the 16 people you spend the most meeting time with. Each row has its own scale.",
    plot: (width) => rows.length && months.length > 1 ? Plot.plot({
      ...plotDefaults(width),
      height: top.length * 34 + 40,
      marginLeft: 150,
      x: { type: "time", label: null },
      y: { axis: null },
      fy: { domain: top.map((p) => p.name), label: null },
      facet: { data: rows, y: "person" },
      marks: [
        Plot.areaY(rows, { x: "month", y: "hours", fy: "person", fill: seriesColor(0), fillOpacity: 0.15, curve: "monotone-x" }),
        Plot.lineY(rows, { x: "month", y: "hours", fy: "person", stroke: seriesColor(0), strokeWidth: 1.5, curve: "monotone-x",
          tip: { format: { x: (d) => d3.timeFormat("%B %Y")(d), y: (h) => `${h.toFixed(1)} h`, fy: true } } }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Person", value: (d) => d.person },
        { label: "Month", value: (d) => d3.timeFormat("%Y-%m")(d.month) },
        { label: "Hours", value: (d) => d.hours.toFixed(1), num: true },
      ],
      rows: rows.filter((r) => r.hours > 0),
    },
  });
}

function internalExternal(body, meetings) {
  if (!MY_DOMAIN) return;
  const kind = (m) => (attending(m).some((i) => domainOf(i.email) !== MY_DOMAIN) ? "With someone outside" : "Only your domain");
  const kinds = ["Only your domain", "With someone outside"];
  const rows = d3.rollups(meetings, (ms) => d3.sum(ms, (m) => m.spokenMinutes) / 60, (m) => d3.timeMonth.floor(m.date), kind)
    .flatMap(([month, byKind]) => byKind.map(([k, hours]) => ({ month, kind: k, hours })));
  const domains = d3.rollups(meetings.flatMap((m) => [...new Set(attending(m).map((i) => domainOf(i.email)))].map((d) => ({ d, m }))),
    (xs) => ({ meetings: xs.length, hours: d3.sum(xs, (x) => x.m.spokenMinutes) / 60 }), (x) => x.d)
    .map(([domain, r]) => ({ domain, ...r })).sort((a, b) => b.meetings - a.meetings);

  figure(body, {
    title: "Inside and outside your organization",
    caption: `Meeting hours per month, split by whether anyone invited was outside ${MY_DOMAIN}.`,
    plot: (width) => rows.length ? Plot.plot({
      ...plotDefaults(width),
      height: 240,
      x: { type: "band", label: null, tickFormat: (d) => d3.timeFormat(d.getMonth() === 0 ? "%b %Y" : "%b")(d) },
      y: { label: "hours" },
      color: { domain: kinds, range: [seriesColor(0), seriesColor(1)], legend: true },
      marks: [
        gridY(),
        Plot.barY(rows, { x: "month", y: "hours", fill: "kind", order: kinds, insetLeft: 2, insetRight: 2,
          stroke: token("--surface"), strokeWidth: 1,
          tip: { format: { x: (d) => d3.timeFormat("%B %Y")(d), y: (h) => `${h.toFixed(1)} h` } } }),
        Plot.ruleY([0], { stroke: token("--axis") }),
      ],
    }) : null,
    table: {
      columns: [
        { label: "Domain", value: (d) => (d.domain === MY_DOMAIN ? `${d.domain} (yours)` : d.domain) },
        { label: "Meetings", value: (d) => d.meetings, num: true },
        { label: "Hours", value: (d) => d.hours.toFixed(1), num: true },
      ],
      rows: domains,
    },
  });
}

function peopleTable(body, people, meetings) {
  const mine = d3.rollup(meetings.filter((m) => m.my_response), (v) => v.length, (m) => m.my_response);
  const card = figure(body, {
    title: "Everyone you meet with",
    caption: `Most meetings first. Accepts: the share of their invitations they accepted. Your own responses to the meetings you recorded: ${fmtInt(mine.get("accepted") || 0)} accepted, ${fmtInt(mine.get("tentative") || 0)} tentative, ${fmtInt(mine.get("needsAction") || 0)} unanswered, ${fmtInt(mine.get("declined") || 0)} declined.`,
  });
  card.append(el("div", { class: "table-wrap" }, renderTable({
    columns: [
      { label: "Person", value: (d) => d.name },
      { label: "Meetings", value: (d) => d.meetings, num: true },
      { label: "Hours", value: (d) => d.hours.toFixed(1), num: true },
      { label: "First", value: (d) => isoDay(d.first) },
      { label: "Last", value: (d) => isoDay(d.last) },
      { label: "1:1s", value: (d) => d.oneOnOnes, num: true },
      { label: "Every", value: (d) => (d.oneOnOneEvery == null ? "–" : `${Math.round(d.oneOnOneEvery)}d`), num: true },
      { label: "Last 1:1", value: (d) => (d.lastOneOnOne ? isoDay(d.lastOneOnOne) : "–") },
      { label: "You in 1:1s", value: (d) => fmtPct(d.myShare), num: true },
      { label: "Accepts", value: (d) => fmtPct(d.acceptRate), num: true },
    ],
    rows: people,
  })));
}
