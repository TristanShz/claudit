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
    // Stacks time (ms), or calls when nothing was hook-timed (`metric`).
    "activities-daily": (data) => {
      const byCalls = data.metric === "calls";
      const value = (s) => (byCalls ? s.calls : s.ms);
      const totals = data.days.map((_, i) => data.series.reduce((a, s) => a + value(s)[i], 0));
      const step = byCalls ? undefined : timeStep(Math.max(1, ...totals));
      const fmt = (v) => (byCalls ? `${v} calls` : fmtMs(v));
      return {
        tooltip: {
          trigger: "axis", axisPointer: { type: "shadow" }, confine: true,
          valueFormatter: fmt,
        },
        legend: { type: "scroll", bottom: 0, textStyle: { color: cssVar("--muted") || "#888" } },
        grid: { left: 8, right: 8, top: 16, bottom: 36, containLabel: true },
        xAxis: { type: "category", data: data.days },
        yAxis: byCalls
          ? { type: "value", minInterval: 1, splitLine: { lineStyle: { opacity: 0.4 } } }
          : {
            type: "value",
            interval: step,
            max: (extent) => Math.max(step, Math.ceil(extent.max / step) * step),
            axisLabel: { formatter: (v) => fmtMs(v) },
            splitLine: { lineStyle: { opacity: 0.4 } },
          },
        series: data.series.map((s) => ({
          name: s.name, type: "bar", stack: "t", barMaxWidth: 32, data: value(s),
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

  // ---- Turn traces --------------------------------------------------------
  // A turn's trace (partials/turn_trace.html, loaded by htmx when a turn is
  // opened): one swimlane per thread (main, then each subagent run), each
  // tool call a bar from execution start to end, its permission wait a pale
  // bar before it, a call without hook timing a tick at its launch; an
  // activity filter that dims the other calls, and the call log below.
  const LOG_PAGE = 500;
  const TRACK = 12, TRACK_GAP = 3, LANE_PAD = 7, MAX_TRACKS = 8;

  function fmtOffset(ms) {
    const sign = ms < 0 ? "-" : "+";
    ms = Math.abs(ms);
    if (ms < 1000) return `${sign}${Math.round(ms)} ms`;
    if (ms < 60000) return `${sign}${(ms / 1000).toFixed(1)} s`;
    const s = Math.floor(ms / 1000), h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
    const pad = (n) => String(n).padStart(2, "0");
    return h ? `${sign}${h}:${pad(m)}:${pad(r)}` : `${sign}${m}:${pad(r)}`;
  }
  function esc(s) {
    return String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  }

  // Tracks within each lane, so parallel calls do not hide each other.
  function layout(data) {
    const minGap = data.span_ms / 400;
    const tracks = data.lanes.map(() => []);
    const trackOf = data.calls.map((c) => {
      const ends = tracks[c.l];
      const end = Math.max(c.at, c.x ? c.x[1] : c.at) + minGap;
      let t = ends.findIndex((e) => e <= c.at);
      if (t < 0) t = ends.length < MAX_TRACKS ? ends.length : ends.indexOf(Math.min(...ends));
      ends[t] = end;
      return t;
    });
    let y = 0;
    const tops = data.lanes.map((_, i) => {
      const top = y;
      const n = Math.max(1, tracks[i].length);
      y += 2 * LANE_PAD + n * TRACK + (n - 1) * TRACK_GAP;
      return top;
    });
    return { trackOf, tops, heights: data.lanes.map((_, i) => (i + 1 < tops.length ? tops[i + 1] : y) - tops[i]), total: y };
  }

  function initTrace(box) {
    if (!window.echarts || box.dataset.ready) return;
    box.dataset.ready = "1";
    const source = document.getElementById(box.dataset.trace);
    const el = box.querySelector(".trace-chart");
    if (!source || !el) return;
    const data = JSON.parse(source.textContent);
    const narrow = el.clientWidth < 600;
    const lay = layout(data);
    const gridTop = 22, gridBottom = 58;
    el.style.height = `${lay.total + gridTop + gridBottom}px`;
    const dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    const chart = echarts.init(el, dark ? "dark" : null, { renderer: "canvas" });
    const colors = data.activities.map(([, slot]) => cssVar(`--act-${slot}`) || "#888");
    const danger = cssVar("--danger") || "#dc2626";
    const text = cssVar("--text") || "#333", muted = cssVar("--muted") || "#888", border = cssVar("--border") || "#ddd";
    const bandColor = cssVar("--series-subagent") || "#eda100";
    let filter = "";
    const matches = (c) => !filter || (filter === ":failed" ? c.st === "failed" : data.activities[c.a][0] === filter);
    const items = () => data.calls.map((c, i) => [
      c.at, c.x ? c.x[1] : c.at,
      lay.tops[c.l] + LANE_PAD + lay.trackOf[i] * (TRACK + TRACK_GAP), i, matches(c) ? 1 : 0,
    ]);
    const laneLabel = (lane) => lane.label;
    const clipRect = (params, r) => echarts.graphic.clipRectByRect(r, {
      x: params.coordSys.x, y: params.coordSys.y, width: params.coordSys.width, height: params.coordSys.height,
    });
    const labelWidth = narrow ? 96 : 190;
    const clipText = (s, px) => {
      const n = Math.max(4, Math.floor(px / 6.4));
      return s.length > n ? s.slice(0, n - 1) + "…" : s;
    };
    const markers = [];
    if (data.turn_start_ms != null) markers.push({ xAxis: data.turn_start_ms, label: { formatter: "prompt" } });
    if (data.turn_end_ms != null) markers.push({ xAxis: data.turn_end_ms, label: { formatter: "stop" } });

    chart.setOption({
      backgroundColor: "transparent",
      animation: false,
      grid: { left: labelWidth + 8, right: 18, top: gridTop, bottom: gridBottom },
      xAxis: {
        type: "value", min: 0, max: data.span_ms,
        axisLabel: { formatter: (v) => fmtOffset(v), hideOverlap: true },
        splitLine: { lineStyle: { opacity: 0.35 } },
      },
      yAxis: { type: "value", min: 0, max: lay.total, inverse: true, show: false },
      dataZoom: [
        { type: "inside", xAxisIndex: 0, filterMode: "none", minValueSpan: 50 },
        { type: "slider", xAxisIndex: 0, filterMode: "none", height: 18, bottom: 10, labelFormatter: (v) => fmtOffset(v), minValueSpan: 50 },
      ],
      tooltip: {
        trigger: "item", confine: true, enterable: false,
        formatter: (p) => {
          if (p.seriesId !== "calls") return "";
          const c = data.calls[p.value[3]];
          const lane = data.lanes[c.l];
          const at = new Date(data.origin_ms + c.at).toLocaleTimeString();
          const rows = [
            `<strong>${esc(c.t)}</strong> <span style="opacity:.7">${esc(data.activities[c.a][0])}</span>`,
            c.s ? `<code style="white-space:pre-wrap;overflow-wrap:anywhere;word-break:break-all">${esc(c.s)}</code>` : "",
            `${esc(lane.label)}`,
            `launched ${fmtOffset(c.at)} (${esc(at)})`,
            c.x ? `ran ${fmtMs(c.x[1] - c.x[0])}, ${fmtOffset(c.x[0])} → ${fmtOffset(c.x[1])}` : "no hook timing: launch instant only",
            c.w ? `waited ${fmtMs(c.w)} for permission` : "",
            c.st === "failed" ? `<span style="color:${danger}">failed</span>${c.e ? `: ${esc(c.e)}` : ""}` : c.st === "open" ? "no result recorded" : "",
            c.sp != null ? `launched subagent: ${esc(data.lanes[c.sp].label)}` : "",
          ];
          return `<div style="max-width:420px;white-space:normal;line-height:1.45">${rows.filter(Boolean).join("<br>")}</div>`;
        },
      },
      series: [
        {
          id: "lanes", type: "custom", silent: true, clip: false, z: 1,
          encode: { x: [0, 1], y: 2 },
          data: data.lanes.map((_, i) => [0, data.span_ms, lay.tops[i], i]),
          renderItem: (params, api) => {
            const i = api.value(3), lane = data.lanes[i];
            const top = api.coord([0, lay.tops[i]])[1];
            const h = api.size([0, lay.heights[i]])[1];
            const cs = params.coordSys;
            const children = [
              { type: "rect", shape: { x: cs.x, y: top, width: cs.width, height: h }, style: { fill: i % 2 ? "transparent" : border, opacity: 0.25 } },
              { type: "line", shape: { x1: cs.x - labelWidth, y1: top + h, x2: cs.x + cs.width, y2: top + h }, style: { stroke: border, lineWidth: 1 } },
              {
                type: "text", x: cs.x - 8, y: top + h / 2,
                style: {
                  text: `{b|${clipText(laneLabel(lane), labelWidth)}}${lane.detail ? `\n{d|${clipText(lane.detail, labelWidth)}}` : ""}`,
                  align: "right", verticalAlign: "middle",
                  rich: { b: { fill: text, fontSize: 12, fontWeight: i ? 400 : 600 }, d: { fill: muted, fontSize: 10, lineHeight: 14 } },
                },
              },
            ];
            for (const [a, b] of lane.spans) {
              const x0 = api.coord([a, 0])[0], x1 = api.coord([b, 0])[0];
              const r = clipRect(params, { x: x0, y: top + 1, width: Math.max(x1 - x0, 1), height: h - 2 });
              if (r) children.push({ type: "rect", shape: r, style: { fill: i ? bandColor : muted, opacity: i ? 0.16 : 0.08 } });
            }
            return { type: "group", children };
          },
        },
        {
          id: "calls", type: "custom", z: 2,
          encode: { x: [0, 1], y: 2 },
          data: items(),
          renderItem: (params, api) => {
            const c = data.calls[api.value(3)];
            const on = api.value(4) === 1;
            const y = api.coord([0, api.value(2)])[1];
            const h = api.size([0, TRACK])[1];
            const color = colors[c.a];
            const failed = c.st === "failed";
            const stroke = failed ? { stroke: danger, lineWidth: 1.5 } : {};
            const children = [];
            if (c.x) {
              const xs = api.coord([c.x[0], 0])[0], xe = api.coord([c.x[1], 0])[0];
              if (c.at < c.x[0]) {
                const xa = api.coord([c.at, 0])[0];
                const r = clipRect(params, { x: xa, y: y + h * 0.3, width: Math.max(xs - xa, 1), height: h * 0.4 });
                if (r) children.push({ type: "rect", shape: r, style: { fill: color, opacity: 0.3 } });
              }
              const r = clipRect(params, { x: xs, y, width: Math.max(xe - xs, 2), height: h });
              if (r) children.push({ type: "rect", shape: r, style: { fill: color, ...stroke } });
            } else {
              const x = api.coord([c.at, 0])[0];
              const r = clipRect(params, { x: x - 1, y: y - 2, width: 2, height: h + 4 });
              if (r) children.push({ type: "rect", shape: r, style: { fill: failed ? danger : color } });
            }
            if (!on) children.forEach((child) => { child.style.opacity = (child.style.opacity ?? 1) * 0.12; });
            return { type: "group", children };
          },
          markLine: markers.length ? {
            silent: true, symbol: "none", animation: false,
            lineStyle: { color: muted, type: "dashed" },
            label: { color: muted, fontSize: 10, position: "start", distance: 2 },
            data: markers,
          } : undefined,
        },
      ],
    });

    // The call log: first LOG_PAGE matching calls, then "Show all".
    const tbody = box.querySelector(".trace-log tbody");
    const more = box.querySelector("button.more");
    let showAll = false;
    const cell = (tr, value, cls) => {
      const td = document.createElement("td");
      if (cls) td.className = cls;
      if (value instanceof Node) td.appendChild(value); else td.textContent = value;
      tr.appendChild(td);
      return td;
    };
    function renderLog() {
      if (!tbody) return;
      const rows = [];
      data.calls.forEach((c, i) => { if (matches(c)) rows.push(i); });
      const shown = showAll ? rows : rows.slice(0, LOG_PAGE);
      const frag = document.createDocumentFragment();
      for (const i of shown) {
        const c = data.calls[i];
        const tr = document.createElement("tr");
        tr.dataset.call = i;
        cell(tr, fmtOffset(c.at), "num");
        cell(tr, data.lanes[c.l].label, "trace-lane");
        const act = document.createElement("span");
        const sw = document.createElement("span");
        sw.className = "swatch";
        sw.style.background = colors[c.a];
        act.append(sw, " ", data.activities[c.a][0]);
        cell(tr, act, "nowrap");
        cell(tr, c.t, "nowrap");
        const td = cell(tr, c.s, "trace-summary");
        td.title = c.s;
        cell(tr, c.x ? fmtMs(c.x[1] - c.x[0]) : "–", "num");
        const st = cell(tr, c.st === "failed" ? "failed" : c.st === "open" ? "no result" : "ok", c.st === "failed" ? "failed" : "muted");
        if (c.e) st.title = c.e;
        frag.appendChild(tr);
      }
      tbody.replaceChildren(frag);
      if (more) {
        more.hidden = showAll || rows.length <= LOG_PAGE;
        more.textContent = `Show all ${rows.length} calls`;
      }
    }
    if (more) more.addEventListener("click", () => { showAll = true; renderLog(); });
    if (tbody) tbody.addEventListener("click", (event) => {
      const tr = event.target.closest("tr[data-call]");
      if (!tr) return;
      const c = data.calls[Number(tr.dataset.call)];
      const end = c.x ? c.x[1] : c.at, pad = Math.max((end - c.at) * 2, data.span_ms / 50, 500);
      chart.dispatchAction({ type: "dataZoom", startValue: Math.max(0, c.at - pad), endValue: Math.min(data.span_ms, end + pad) });
      el.scrollIntoView({ block: "nearest", behavior: "smooth" });
    });
    box.querySelectorAll(".trace-chips .chip").forEach((chip) => chip.addEventListener("click", () => {
      filter = chip.dataset.activity;
      box.querySelectorAll(".trace-chips .chip").forEach((c) => c.setAttribute("aria-pressed", String(c === chip)));
      chart.setOption({ series: [{ id: "calls", data: items() }] });
      renderLog();
    }));
    renderLog();
  }

  function initTraces(root) {
    root.querySelectorAll("[data-trace]").forEach(initTrace);
  }

  // Tabs (`[data-tabs]`): each `[data-tab]` button shows the elements of
  // its `[data-panel]` and hides the others'. One delegated listener.
  document.addEventListener("click", (event) => {
    const tab = event.target.closest && event.target.closest("[data-tabs] [data-tab]");
    if (!tab) return;
    const box = tab.closest("[data-tabs]");
    box.querySelectorAll("[data-tab]").forEach((t) => {
      const on = t === tab;
      t.setAttribute("aria-selected", String(on));
      t.setAttribute("aria-pressed", String(on));
    });
    box.querySelectorAll("[data-panel]").forEach((p) => { p.hidden = p.dataset.panel !== tab.dataset.tab; });
  });

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

  document.addEventListener("DOMContentLoaded", () => { renderCharts(document); initTraces(document); });
  document.addEventListener("htmx:after:settle", () => { renderCharts(document); initTraces(document); });
  window.addEventListener("resize", () =>
    document.querySelectorAll("[data-chart]").forEach((el) => echarts.getInstanceByDom(el)?.resize())
  );
})();
