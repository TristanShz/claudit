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
  };

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
