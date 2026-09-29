// Renders every `[data-chart]` element with ECharts, reading its data from
// the JSON <script> named by `data-source`. Runs on load and after htmx swaps.
(function () {
  const renderers = {
    "tools-bar": (rows) => ({
      tooltip: { trigger: "axis", axisPointer: { type: "shadow" } },
      grid: { left: 8, right: 16, top: 8, bottom: 8, containLabel: true },
      xAxis: { type: "value", name: "calls" },
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
    "time-daily": (data) => ({
      tooltip: {
        trigger: "axis", axisPointer: { type: "shadow" },
        valueFormatter: (v) => fmtMs(v),
      },
      grid: { left: 8, right: 8, top: 16, bottom: 8, containLabel: true },
      xAxis: { type: "category", data: data.days.map((d) => d.day) },
      yAxis: { type: "value", axisLabel: { formatter: (v) => fmtMs(v) }, splitLine: { lineStyle: { opacity: 0.4 } } },
      series: data.split.map((p) => ({
        name: p.label, type: "bar", stack: "t", barMaxWidth: 32,
        data: data.days.map((d) => d[p.kind]),
        itemStyle: { color: seriesColor(p.kind), borderColor: surface(), borderWidth: 1 },
      })),
    }),
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
      chart.setOption(Object.assign({ backgroundColor: "transparent" }, render(JSON.parse(source.textContent))));
    });
  }

  document.addEventListener("DOMContentLoaded", () => renderCharts(document));
  document.addEventListener("htmx:after:settle", () => renderCharts(document));
  window.addEventListener("resize", () =>
    document.querySelectorAll("[data-chart]").forEach((el) => echarts.getInstanceByDom(el)?.resize())
  );
})();
