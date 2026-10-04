// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors
// Dashboard renderer (ROADMAP M1.64): vanilla JS and inline SVG, no library, no network.
// build_dashboard.py inlines this file, dashboard.css and the data into one HTML file.
(function () {
  "use strict";
  var DATA = JSON.parse(document.getElementById("data").textContent);
  var NS = "http://www.w3.org/2000/svg";

  function el(tag, attrs, kids) {
    var e = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (k) {
      if (k === "text") e.textContent = attrs[k];
      else e.setAttribute(k, attrs[k]);
    });
    (kids || []).forEach(function (c) { e.appendChild(c); });
    return e;
  }
  function sv(tag, attrs, text) {
    var e = document.createElementNS(NS, tag);
    Object.keys(attrs || {}).forEach(function (k) { e.setAttribute(k, attrs[k]); });
    if (text != null) e.textContent = text;
    return e;
  }
  function pct(v, d) { return v == null ? "n/a" : (v * 100).toFixed(d == null ? 1 : d) + "%"; }
  function color(i) { return "var(--c" + (i % 8) + ")"; }
  function label(s) { return s.suite + " / " + s.predictor; }

  // "Nice" tick values covering [lo, hi].
  function ticks(lo, hi, count) {
    if (!(hi > lo)) { var pad = Math.abs(lo) * 0.05 || 1; lo -= pad; hi += pad; }
    var raw = (hi - lo) / count, mag = Math.pow(10, Math.floor(Math.log10(raw)));
    var step = [1, 2, 2.5, 5, 10].map(function (m) { return m * mag; })
      .filter(function (s) { return s >= raw; })[0];
    var out = [];
    for (var v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) out.push(+v.toFixed(10));
    return out;
  }
  function fmtDate(ms) { return new Date(ms).toISOString().slice(0, 10); }

  // A line or scatter chart. lines: [{label, color, pts:[{x,y,tip}], dots:boolean}]. x is time (ms)
  // unless opts.xnum, y a percentage. Returns an <svg>.
  function chart(lines, opts) {
    var W = opts.width || 640, H = opts.height || 260, L = 54, R = 14, T = 12, B = 34;
    var all = [];
    lines.forEach(function (l) { l.pts.forEach(function (p) { all.push(p); }); });
    var svg = sv("svg", { viewBox: "0 0 " + W + " " + H, role: "img", "aria-label": opts.title });
    if (!all.length) {
      svg.appendChild(sv("text", { x: W / 2, y: H / 2, "text-anchor": "middle", class: "muted" }, "no data yet"));
      return svg;
    }
    var xs = all.map(function (p) { return p.x; }), ys = all.map(function (p) { return p.y; });
    var x0 = Math.min.apply(null, xs), x1 = Math.max.apply(null, xs);
    var y0 = Math.min.apply(null, ys), y1 = Math.max.apply(null, ys);
    if (opts.xnum) { x0 = Math.min(0, x0); x1 = x1 * 1.1 || 1; }
    else if (x0 === x1) { x0 -= 864e5; x1 += 864e5; }
    var yt = ticks(y0, y1, 5);
    y0 = Math.min(y0, yt[0]); y1 = Math.max(y1, yt[yt.length - 1]);
    var xt = opts.xnum ? ticks(x0, x1, 5) : null;
    function X(v) { return L + (v - x0) / (x1 - x0) * (W - L - R); }
    function Y(v) { return H - B - (v - y0) / (y1 - y0 || 1) * (H - T - B); }
    yt.forEach(function (v) {
      svg.appendChild(sv("line", { x1: L, x2: W - R, y1: Y(v), y2: Y(v), class: "grid" }));
      svg.appendChild(sv("text", { x: L - 6, y: Y(v) + 4, "text-anchor": "end", class: "tick" }, v.toFixed(v % 1 ? 1 : 0) + (opts.unit || "")));
    });
    if (opts.xnum) {
      xt.forEach(function (v) {
        svg.appendChild(sv("text", { x: X(v), y: H - B + 16, "text-anchor": "middle", class: "tick" }, String(v)));
      });
    } else {
      var n = Math.min(5, Math.max(2, Math.round((x1 - x0) / 864e5) + 1));
      for (var i = 0; i < n; i++) {
        var t = x0 + (x1 - x0) * (n === 1 ? 0.5 : i / (n - 1));
        svg.appendChild(sv("text", { x: X(t), y: H - B + 16, "text-anchor": i === 0 ? "start" : i === n - 1 ? "end" : "middle", class: "tick" }, fmtDate(t)));
      }
    }
    svg.appendChild(sv("line", { x1: L, x2: L, y1: T, y2: H - B, class: "axis" }));
    svg.appendChild(sv("line", { x1: L, x2: W - R, y1: H - B, y2: H - B, class: "axis" }));
    if (opts.xlabel) svg.appendChild(sv("text", { x: (L + W - R) / 2, y: H - 2, "text-anchor": "middle", class: "tick" }, opts.xlabel));
    lines.forEach(function (l) {
      if (!opts.scatter && l.pts.length > 1) {
        svg.appendChild(sv("polyline", {
          points: l.pts.map(function (p) { return X(p.x).toFixed(1) + "," + Y(p.y).toFixed(1); }).join(" "),
          fill: "none", stroke: l.color, "stroke-width": 2, "stroke-linejoin": "round"
        }));
      }
      l.pts.forEach(function (p, idx) {
        var latest = idx === l.pts.length - 1;
        var c = sv("circle", { cx: X(p.x), cy: Y(p.y), r: opts.scatter ? (latest ? 7 : 4) : (l.pts.length > 1 ? 3 : 5), fill: l.color, class: "dot", tabindex: 0 });
        if (opts.scatter && !latest) c.setAttribute("opacity", "0.4");
        c.appendChild(sv("title", {}, l.label + "\n" + p.tip));
        svg.appendChild(c);
      });
      if (opts.scatter) {
        var q = l.pts[l.pts.length - 1];
        svg.appendChild(sv("text", { x: X(q.x) + 9, y: Y(q.y) + 4, class: "tick strong" }, l.label));
      }
    });
    return svg;
  }

  function legend(items) {
    return el("div", { class: "legend" }, items.map(function (it) {
      var sw = el("span", { class: "sw" }); sw.style.background = it.color;
      return el("span", { class: "key" }, [sw, document.createTextNode(it.label)]);
    }));
  }

  function card(title, note, body) {
    var k = [el("h2", { text: title })];
    if (note) k.push(el("p", { class: "note", text: note }));
    return el("section", { class: "card" }, k.concat(body));
  }

  var series = DATA.series;
  series.forEach(function (s, i) { s.color = color(i); });
  var root = document.getElementById("app");

  // 1. Gate table.
  (function () {
    var rows = DATA.gates.map(function (g) {
      return el("tr", {}, [
        el("td", { text: g.name }),
        el("td", { class: "st " + g.status }, [el("span", { class: "badge " + g.status, text: g.status })]),
        el("td", { text: g.detail })
      ]);
    });
    var tbl = el("table", {}, [el("thead", {}, [el("tr", {}, [el("th", { text: "Gate" }), el("th", { text: "Status" }), el("th", { text: "Measured" })])]), el("tbody", {}, rows)]);
    root.appendChild(card("Gate table", "Statuses: pass, fail, pending (not measurable yet), info (reported, no bar). A missed gate goes back to the owner; nothing here relaxes a bar.", [el("div", { class: "scroll" }, [tbl])]));
  })();

  // 2. Headline trends: one small chart per series, so a 98% series does not flatten a 27% one.
  (function () {
    var grid = el("div", { class: "grid3" }, series.map(function (s) {
      function line(field, name, col) {
        return { label: name, color: col, pts: s.points.filter(function (p) { return p[field] != null; }).map(function (p) {
          return { x: Date.parse(p.t), y: p[field] * 100, tip: fmtDate(Date.parse(p.t)) + "  commit " + p.commit + "\n" + name + " " + pct(p[field], 2) + "  (n = " + p.n + ")" };
        }) };
      }
      var ls = [line("quality", "quality", "var(--c0)"), line("failure", "failure rate", "var(--c1)")];
      return el("div", {}, [el("h3", { text: label(s) }), legend(ls.map(function (l) { return { label: l.label, color: l.color }; })), chart(ls, { title: label(s) + " trend", unit: "", height: 200 })]);
    }));
    root.appendChild(card("Nightly trends", "One point per nightly record on the metrics branch, %. Quality is the mean IoU (mean matched IoU for the multi-item suite); the failure rate counts IoU < 0.9 (scans that are not perfect for multi-item).", [grid]));
  })();

  // 3. Latency vs failure.
  (function () {
    var timed = series.filter(function (s) { return s.points.some(function (p) { return p.ms_per_item != null; }); });
    var ls = timed.map(function (s) {
      var pts = s.points.filter(function (p) { return p.ms_per_item != null && p.failure != null; });
      return { label: label(s), color: s.color, pts: pts.map(function (p) {
        return { x: p.ms_per_item, y: p.failure * 100, tip: fmtDate(Date.parse(p.t)) + "  commit " + p.commit + "\n" + p.ms_per_item.toFixed(2) + " ms per item, failure " + pct(p.failure, 2) };
      }) };
    });
    root.appendChild(card("Latency against failure rate", "Wall-clock milliseconds per image (or scan) of the whole `eval run` on the shared CI runner, x the failure rate; lower left is better. The latest point of a series carries its name. Shared-runner timings are noisy: read the shape, not the digits.", [
      chart(ls, { title: "latency vs failure", scatter: true, xnum: true, xlabel: "ms per item (wall, shared runner)", unit: "", width: 960, height: 300 })
    ]));
  })();

  // 4. Per-slice trends.
  (function () {
    var host = el("div", {});
    var sSel = el("select", { "aria-label": "series" }, series.map(function (s, i) { return el("option", { value: i, text: label(s) }); }));
    var aSel = el("select", { "aria-label": "axis" });
    var mSel = el("select", { "aria-label": "metric" }, [el("option", { value: "quality", text: "quality" }), el("option", { value: "failure", text: "failure rate" })]);
    function axesOf(s) {
      var set = {};
      s.points.forEach(function (p) { Object.keys(p.slices).forEach(function (k) { set[k.split("=")[0]] = 1; }); });
      return Object.keys(set).sort();
    }
    function fillAxes() {
      var s = series[+sSel.value]; aSel.innerHTML = "";
      axesOf(s).forEach(function (a) { aSel.appendChild(el("option", { value: a, text: a })); });
    }
    function draw() {
      host.innerHTML = "";
      var s = series[+sSel.value]; if (!s || !aSel.value) { host.appendChild(el("p", { class: "muted", text: "no slices yet" })); return; }
      var m = mSel.value, axis = aSel.value + "=", keys = {};
      s.points.forEach(function (p) { Object.keys(p.slices).forEach(function (k) { if (k.indexOf(axis) === 0) keys[k] = 1; }); });
      var names = Object.keys(keys).sort();
      var ls = names.map(function (k, i) {
        return { label: k.slice(axis.length), color: color(i), pts: s.points.filter(function (p) { return p.slices[k] && p.slices[k][m] != null; }).map(function (p) {
          return { x: Date.parse(p.t), y: p.slices[k][m] * 100, tip: fmtDate(Date.parse(p.t)) + "  n = " + p.slices[k].n + "\n" + k + ": " + pct(p.slices[k][m], 2) };
        }) };
      });
      host.appendChild(legend(ls.map(function (l) { return { label: l.label, color: l.color }; })));
      host.appendChild(chart(ls, { title: "slice trend", unit: "", width: 960, height: 300 }));
      var last = s.points[s.points.length - 1], first = s.points[0];
      var rows = names.map(function (k) {
        var a = last.slices[k], b = first.slices[k];
        var d = a && b && a[m] != null && b[m] != null ? (a[m] - b[m]) * 100 : null;
        return el("tr", {}, [el("td", { text: k }), el("td", { text: a ? String(a.n) : "n/a" }), el("td", { text: a ? pct(a.quality, 1) : "n/a" }), el("td", { text: a ? pct(a.failure, 1) : "n/a" }), el("td", { text: d == null ? "n/a" : (d >= 0 ? "+" : "") + d.toFixed(2) + " pt" })]);
      });
      host.appendChild(el("div", { class: "scroll" }, [el("table", {}, [el("thead", {}, [el("tr", {}, ["slice", "n", "quality", "failure rate", "change since first record (selected metric)"].map(function (t) { return el("th", { text: t }); }))]), el("tbody", {}, rows)])]));
    }
    sSel.addEventListener("change", function () { fillAxes(); draw(); });
    aSel.addEventListener("change", draw); mSel.addEventListener("change", draw);
    fillAxes(); draw();
    root.appendChild(card("Per-slice trends", "Slices with n >= 30 only (smaller ones are never published). Each tag value is a slice; n = 30 to 79 is advisory, 80 and up is gated.", [el("div", { class: "controls" }, [sSel, aSel, mSel]), host]));
  })();

  // 5. Latest numbers.
  (function () {
    var rows = series.map(function (s) {
      var p = s.points[s.points.length - 1];
      return el("tr", {}, [el("td", { text: label(s) }), el("td", { text: fmtDate(Date.parse(p.t)) + "  " + p.commit }), el("td", { text: String(p.n) }), el("td", { text: pct(p.quality, 2) }), el("td", { text: pct(p.failure, 2) }), el("td", { text: p.silent_risk_ub95 == null ? "n/a" : pct(p.silent_risk_ub95, 2) }), el("td", { text: p.ms_per_item == null ? "n/a" : p.ms_per_item.toFixed(2) }), el("td", { text: p.host }), el("td", { text: String(s.points.length) })]);
    });
    var head = ["series", "latest", "n", "quality", "failure rate", "silent-failure bound (95%)", "ms per item", "host", "records"];
    root.appendChild(card("Latest records", "", [el("div", { class: "scroll" }, [el("table", {}, [el("thead", {}, [el("tr", {}, head.map(function (t) { return el("th", { text: t }); }))]), el("tbody", {}, rows)])])]));
  })();
})();
