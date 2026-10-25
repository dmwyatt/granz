"use strict";

// The data grans embedded: { generated_at, meetings: [MeetingRow, ...] }.
// Field meanings are documented on the Rust types in src/stats/.
const DATA = JSON.parse(document.getElementById("grans-data").textContent);
const GENERATED = new Date(DATA.generated_at);
const DAY = 86400000;

function sumChars(voices, keep) {
  return voices.filter(keep).reduce((total, v) => total + v.chars, 0);
}

const isOthers = (v) => v.voice === "named" || v.voice === "others";

// Each row with the derived values every section uses.
const MEETINGS = DATA.meetings.map((m) => {
  const t = m.transcript;
  const voices = t ? t.voices : [];
  // Stretches of talk; a note reopened later for another talk has several.
  const sessions = t
    ? t.sessions.map((s) => ({ start: new Date(s.start), end: new Date(s.end) }))
    : [];
  return {
    ...m,
    date: new Date(m.created_at),
    sessions,
    spokenMinutes: d3.sum(sessions, (s) => (s.end - s.start) / 60000),
    myChars: sumChars(voices, (v) => v.voice === "me"),
    othersChars: sumChars(voices, isOthers),
    namedSpeakers: voices.filter((v) => v.voice === "named").length,
    invited: m.invitees.filter((i) => i.response !== "declined").length,
  };
});

// When Granola started naming speakers, from the data. Named-speaker charts
// cover only meetings since then; time charts mark it.
const NAMED_SINCE = (() => {
  const named = MEETINGS.filter((m) => m.namedSpeakers > 0).map((m) => m.date);
  return named.length ? new Date(Math.min(...named)) : null;
})();

// A recurring series is labelled by its most recent instance's title.
const SERIES_TITLES = new Map();
for (const m of MEETINGS) if (m.series) SERIES_TITLES.set(m.series, m.title);

// A person is labelled by the name they most often appear under.
const PEOPLE = (() => {
  const names = new Map();
  for (const m of MEETINGS) {
    for (const i of m.invitees) {
      const counts = names.get(i.email) || new Map();
      if (i.name) counts.set(i.name, (counts.get(i.name) || 0) + 1);
      names.set(i.email, counts);
    }
  }
  const label = new Map();
  for (const [email, counts] of names) {
    const best = [...counts].sort((a, b) => b[1] - a[1])[0];
    label.set(email, best ? best[0] : email);
  }
  return label;
})();

const personName = (email) => PEOPLE.get(email) || email;

// Hours in one session of talk.
const hoursOf = (session) => (session.end - session.start) / 3600000;

// ---------------------------------------------------------------------------
// Filters

const state = { from: null, to: null, series: "", person: "", preset: "all" };

function inView(m) {
  if (state.from && m.date < state.from) return false;
  if (state.to && m.date >= state.to) return false;
  if (state.series && m.series !== state.series) return false;
  if (state.person && !m.invitees.some((i) => i.email === state.person)) return false;
  return true;
}

const PRESETS = [
  ["all", "All time", null],
  ["365", "12 months", 365],
  ["90", "90 days", 90],
  ["30", "30 days", 30],
];

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value);
  }
  for (const child of children.flat()) {
    if (child != null) node.append(child instanceof Node ? child : String(child));
  }
  return node;
}

function option(value, label, selected) {
  const o = el("option", { value }, label);
  o.selected = value === selected;
  return o;
}

function dateInputValue(date) {
  return date ? isoDay(date) : "";
}

function buildFilters() {
  const nav = document.getElementById("filters");
  nav.replaceChildren();

  const presets = el("div", { class: "presets", role: "group", "aria-label": "Period" });
  for (const [key, label, days] of PRESETS) {
    presets.append(
      el("button", {
        type: "button",
        "aria-pressed": String(state.preset === key),
        onclick: () => {
          state.preset = key;
          state.from = days ? new Date(GENERATED - days * DAY) : null;
          state.to = null;
          update();
        },
      }, label),
    );
  }

  const dateInput = (label, key) =>
    el("label", {}, label,
      el("input", {
        type: "date",
        value: dateInputValue(state[key]),
        onchange: (e) => {
          state[key] = e.target.value ? new Date(e.target.value + "T00:00:00") : null;
          state.preset = "custom";
          update();
        },
      }));

  const seriesOptions = [...SERIES_TITLES]
    .sort((a, b) => a[1].localeCompare(b[1]))
    .map(([id, title]) => option(id, title, state.series));
  const personOptions = [...PEOPLE]
    .sort((a, b) => a[1].localeCompare(b[1]))
    .map(([email, name]) => option(email, name, state.person));

  nav.append(
    presets,
    dateInput("From", "from"),
    dateInput("Before", "to"),
    el("label", {}, "Series",
      el("select", { onchange: (e) => { state.series = e.target.value; update(); } },
        option("", "All meetings", state.series), seriesOptions)),
    el("label", {}, "Invitee",
      el("select", { onchange: (e) => { state.person = e.target.value; update(); } },
        option("", "Anyone", state.person), personOptions)),
    el("span", { class: "count", id: "count" }),
  );
}

// ---------------------------------------------------------------------------
// Formatting

const fmtInt = (n) => Math.round(n).toLocaleString();
const fmtPct = (f) => (f == null || Number.isNaN(f) ? "–" : `${Math.round(f * 100)}%`);
const fmtHours = (minutes) => (minutes / 60).toLocaleString(undefined, { maximumFractionDigits: 1 });

function isoDay(date) {
  const pad = (n) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function share(part, rest) {
  return part + rest > 0 ? part / (part + rest) : null;
}

// ---------------------------------------------------------------------------
// Colors come from the CSS tokens, so light and dark mode are both the
// stylesheet's choice.

function token(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

const SERIES_TOKENS = [1, 2, 3, 4, 5, 6, 7, 8].map((n) => `--series-${n}`);
const seriesColor = (slot) => token(SERIES_TOKENS[slot]);

// Plot options every chart shares.
function plotDefaults(width) {
  return {
    width,
    marginLeft: 48,
    marginTop: 28,
    style: { background: "transparent", color: token("--ink-2"), fontSize: "12px" },
  };
}

function gridY() {
  return Plot.gridY({ stroke: token("--grid"), strokeOpacity: 1 });
}

// A vertical rule where speaker names begin, for charts over time. The
// label sits on whichever side of the rule has more room.
function eraMarks(domain) {
  if (!NAMED_SINCE || (domain && (NAMED_SINCE < domain[0] || NAMED_SINCE > domain[1]))) return [];
  const late = domain && (NAMED_SINCE - domain[0]) / (domain[1] - domain[0]) > 0.6;
  return [
    Plot.ruleX([NAMED_SINCE], { stroke: token("--muted"), strokeWidth: 1 }),
    Plot.text([NAMED_SINCE], {
      x: (d) => d,
      text: () => "speaker names from here",
      frameAnchor: "top",
      textAnchor: late ? "end" : "start",
      lineAnchor: "bottom",
      dx: late ? -4 : 4,
      dy: -6,
      fill: token("--muted"),
    }),
  ];
}

// ---------------------------------------------------------------------------
// Sections and figures

const SECTIONS = [];

// A section of the page: render(meetings, body) fills body for the meetings
// the filters select.
function section(id, title, lede, render) {
  SECTIONS.push({ id, title, lede, render });
}

// Charts are drawn after every section is laid out, so each is measured at
// the width it will actually have.
const PENDING_DRAWS = [];

function drawPending() {
  for (const draw of PENDING_DRAWS.splice(0)) draw();
}

// A chart card with its table twin. `plot(width)` returns an SVG (or Plot's
// figure, when it has a legend) or null when there is nothing to draw;
// `table` is { columns, rows } where each column is
// { label, value(row), num }.
function figure(parent, { title, caption, plot, table }) {
  const card = el("figure", { class: "card" }, el("h3", {}, title), caption ? el("figcaption", {}, caption) : null);
  const holder = el("div", { class: "plot" });
  card.append(holder);
  parent.append(card);

  if (plot) {
    PENDING_DRAWS.push(() => {
      const chart = plot(Math.max(280, holder.clientWidth));
      holder.append(chart || el("div", { class: "empty" }, "Nothing to show for this selection."));
    });
  }

  if (table && table.rows.length) {
    card.append(el("details", {}, el("summary", {}, plot ? "Table" : `Show ${table.rows.length} rows`),
      el("div", { class: "table-wrap" }, renderTable(table))));
  }
  return card;
}

function renderTable({ columns, rows }) {
  const head = el("tr", {}, columns.map((c) => el("th", c.num ? { class: "num" } : {}, c.label)));
  const body = rows.map((row) =>
    el("tr", {}, columns.map((c) => el("td", c.num ? { class: "num" } : {}, c.value(row)))));
  return el("table", {}, el("thead", {}, head), el("tbody", {}, body));
}

function tiles(parent, items) {
  parent.append(el("div", { class: "tiles" },
    items.map(({ label, value, detail }) =>
      el("div", { class: "tile" },
        el("div", { class: "label" }, label),
        el("div", { class: "value" }, value),
        detail ? el("div", { class: "detail" }, detail) : null))));
}
