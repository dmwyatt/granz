section("overview", "Overview", null, (meetings, body) => {
  const transcribed = meetings.filter((m) => m.transcript);
  const minutes = transcribed.map((m) => m.spokenMinutes).sort((a, b) => a - b);
  const mine = transcribed.reduce((t, m) => t + m.myChars, 0);
  const others = transcribed.reduce((t, m) => t + m.othersChars, 0);
  const named = meetings.filter((m) => m.namedSpeakers > 0).length;

  tiles(body, [
    { label: "Meetings", value: fmtInt(meetings.length),
      detail: `${fmtInt(transcribed.length)} with a transcript` },
    { label: "Spoken time", value: `${fmtHours(minutes.reduce((a, b) => a + b, 0))} h`,
      detail: "first word to last" },
    { label: "Typical meeting", value: minutes.length ? `${fmtInt(d3.median(minutes))} min` : "–",
      detail: "median spoken time" },
    { label: "Your share", value: fmtPct(share(mine, others)), detail: "of what was said" },
    { label: "Named speakers", value: fmtInt(named),
      detail: NAMED_SINCE ? `meetings, since ${isoDay(NAMED_SINCE)}` : "none detected" },
  ]);
});
