// Renders every `[data-chart]` element with ECharts, reading its data from
// the JSON <script> named by `data-source`. Runs on load and after htmx swaps.
(function () {
  const renderers = {
    "tools-bar": (rows) => ({
      tooltip: { trigger: "axis", axisPointer: { type: "shadow" } },
      grid: { left: 8, right: 16, top: 8, bottom: 8, containLabel: true },
      xAxis: { type: "value", minInterval: 1 },
      yAxis: { type: "category", inverse: true, data: rows.map((r) => r.name) },
      series: [{ type: "bar", data: rows.map((r) => r.calls), itemStyle: { color: "#c2410c" } }],
    }),
    // Where the time goes: one stacked bar (model / tools / waiting / subagents).
    "time-split": (data) => ({
      tooltip: { trigger: "item", formatter: (p) => `${p.seriesName}: ${fmtMs(p.value)}` },
      grid: { left: 0, right: 0, top: 0, bottom: 0 },
      xAxis: { type: "value", show: false, max: Math.max(1, data.split.reduce((a, p) => a + p.ms, 0)) },
      yAxis: { type: "category", show: false, data: ["time"] },
      series: data.split.map((p) => ({
        name: p.label, type: "bar", stack: "t", barWidth: 22, data: [p.ms],
        itemStyle: { color: seriesColor(p.kind), borderColor: surface(), borderWidth: 1 },
      })),
    }),
    // The same split per day, as stacked columns.
    "time-daily": (data) => {
      const dailyStep = timeStep(Math.max(1, ...data.days.map((d) => d.model + d.tool + d.waiting + d.subagent)));
      return {
        tooltip: {
          trigger: "axis", axisPointer: { type: "shadow" },
          valueFormatter: (v) => fmtMs(v),
        },
        grid: { left: 8, right: 8, top: 16, bottom: 8, containLabel: true },
        xAxis: { type: "category", data: data.days.map((d) => d.day) },
        yAxis: {
          type: "value",
          interval: dailyStep,
          max: (extent) => Math.max(dailyStep, Math.ceil(extent.max / dailyStep) * dailyStep),
          axisLabel: { formatter: (v) => fmtMs(v) },
          splitLine: { lineStyle: { opacity: 0.4 } },
        },
        series: data.split.map((p) => ({
          name: p.label, type: "bar", stack: "t", barMaxWidth: 32,
          data: data.days.map((d) => d[p.kind]),
          itemStyle: { color: seriesColor(p.kind), borderColor: surface(), borderWidth: 1 },
        })),
      };
    },
    // Tool time per activity and day, as stacked columns.
    "activities-daily": (data) => {
      const totals = data.days.map((_, i) => data.series.reduce((a, s) => a + s.ms[i], 0));
      const step = timeStep(Math.max(1, ...totals));
      return {
        tooltip: {
          trigger: "axis", axisPointer: { type: "shadow" }, confine: true,
          valueFormatter: (v) => fmtMs(v),
        },
        legend: { type: "scroll", bottom: 0, textStyle: { color: cssVar("--muted") || "#888" } },
        grid: { left: 8, right: 8, top: 16, bottom: 36, containLabel: true },
        xAxis: { type: "category", data: data.days },
        yAxis: {
          type: "value",
          interval: step,
          max: (extent) => Math.max(step, Math.ceil(extent.max / step) * step),
          axisLabel: { formatter: (v) => fmtMs(v) },
          splitLine: { lineStyle: { opacity: 0.4 } },
        },
        series: data.series.map((s) => ({
          name: s.name, type: "bar", stack: "t", barMaxWidth: 32, data: s.ms,
          itemStyle: { color: cssVar(`--act-${s.color}`) || "#888", borderColor: surface(), borderWidth: 1 },
        })),
      };
    },
    // Session turn timeline: one lane per turn, its model / tool / waiting /
    // subagent segments drawn as rectangles positioned in time from the
    // turn's start (a custom series).
    "turn-timeline": (data, el) => {
      const narrow = el.clientWidth < 600;
      const colors = data.kinds.map((k) => seriesColor(k.kind));
      const lanes = data.turns.map((t) => t.label);
      const items = [];
      data.turns.forEach((turn, lane) =>
        turn.segments.forEach(([kind, start, end]) => items.push([lane, start, end, kind]))
      );
      const maxMs = Math.max(1, ...data.turns.map((t) => t.duration_ms));
      const clip = (text, n) => {
        const flat = text.replace(/\s+/g, " ").trim();
        return flat.length > n ? flat.slice(0, n - 1) + "…" : flat;
      };
      return {
        tooltip: {
          trigger: "item",
          confine: true,
          formatter: (p) => {
            const [lane, start, end, kind] = p.value;
            const turn = data.turns[lane];
            const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
            return `<div style="max-width:360px;white-space:normal">`
              + `<strong>${esc(data.kinds[kind].label)}</strong> ${fmtMs(end - start)}`
              + `<br><span style="opacity:.7">${fmtMs(start)} → ${fmtMs(end)} into turn ${turn.label}</span>`
              + `<br>${turn.label} · ${fmtMs(turn.duration_ms)} · ${esc(turn.started)}`
              + (turn.prompt ? `<br><em>${esc(clip(turn.prompt, 160))}</em>` : "")
              + `</div>`;
          },
        },
        grid: { left: 8, right: 24, top: 8, bottom: 8, containLabel: true },
        xAxis: {
          type: "value", min: 0, max: maxMs, interval: timeStep(maxMs, Math.max(2, Math.floor((el.clientWidth - (narrow ? 120 : 220)) / 90))),
          axisLabel: { formatter: (v) => fmtMs(v) },
          splitLine: { lineStyle: { opacity: 0.4 } },
        },
        yAxis: {
          type: "category", inverse: true, data: lanes,
          axisTick: { show: false },
          axisLabel: {
            formatter: (label, i) => {
              const turn = data.turns[i];
              return `{b|${label}} {d|${fmtMs(turn.duration_ms)}}\n{p|${clip(turn.prompt || "", narrow ? 14 : 28)}}`;
            },
            rich: {
              b: { fontWeight: "bold" },
              d: { color: cssVar("--muted") || "#888" },
              p: { color: cssVar("--muted") || "#888", fontSize: 11, lineHeight: 16 },
            },
          },
        },
        series: [{
          type: "custom",
          encode: { x: [1, 2], y: 0 },
          data: items,
          renderItem: (params, api) => {
            const lane = api.value(0);
            const start = api.coord([api.value(1), lane]);
            const end = api.coord([api.value(2), lane]);
            const height = api.size([0, 1])[1] * 0.56;
            const shape = echarts.graphic.clipRectByRect(
              { x: start[0], y: start[1] - height / 2, width: Math.max(end[0] - start[0], 1), height },
              { x: params.coordSys.x, y: params.coordSys.y, width: params.coordSys.width, height: params.coordSys.height }
            );
            return shape && {
              type: "rect", shape, transition: ["shape"],
              style: { fill: colors[api.value(3)] },
            };
          },
        }],
      };
    },
  };

  function cssVar(name) {
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  }
  function seriesColor(kind) {
    return cssVar(`--series-${kind}`) || "#888";
  }
  function surface() {
    return cssVar("--surface") || "#fff";
  }
  // A round time step giving at most `ticks` (default 7) ticks over `maxMs`.
  function timeStep(maxMs, ticks = 7) {
    const steps = [100, 200, 500, 1e3, 2e3, 5e3, 1e4, 15e3, 3e4, 6e4, 12e4, 3e5, 6e5, 9e5, 18e5, 36e5, 72e5, 144e5, 216e5, 432e5, 864e5];
    return steps.find((s) => maxMs / s <= ticks) || Math.ceil(maxMs / ticks / 36e5) * 36e5;
  }
  function fmtMs(ms) {
    if (ms < 1000) return `${Math.round(ms)} ms`;
    if (ms < 60000) return `${(ms / 1000).toFixed(1)} s`;
    if (ms < 3600000) return `${Math.floor(ms / 60000)} min ${String(Math.floor((ms % 60000) / 1000)).padStart(2, "0")} s`;
    return `${Math.floor(ms / 3600000)} h ${String(Math.floor((ms % 3600000) / 60000)).padStart(2, "0")} min`;
  }

  function renderCharts(root) {
    if (!window.echarts) return;
    const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    root.querySelectorAll("[data-chart]").forEach((el) => {
      const render = renderers[el.dataset.chart];
      const source = document.getElementById(el.dataset.source);
      if (!render || !source) return;
      const existing = echarts.getInstanceByDom(el);
      if (existing) existing.dispose();
      const chart = echarts.init(el, dark ? "dark" : null, { renderer: "svg" });
      chart.setOption(Object.assign({ backgroundColor: "transparent" }, render(JSON.parse(source.textContent), el)));
    });
  }

  // Refresh runs an ingest before reloading: show that it is working.
  document.addEventListener("submit", (event) => {
    const form = event.target;
    if (!form.matches || !form.matches("form.refresh")) return;
    const button = form.querySelector("button");
    if (button) {
      button.disabled = true;
      button.textContent = "Refreshing…";
    }
  });

  document.addEventListener("DOMContentLoaded", () => renderCharts(document));
  document.addEventListener("htmx:after:settle", () => renderCharts(document));
  window.addEventListener("resize", () =>
    document.querySelectorAll("[data-chart]").forEach((el) => echarts.getInstanceByDom(el)?.resize())
  );
})();
