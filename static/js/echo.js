// echo.js — minibuffer echo runtime for Wave Funk.
//
// API:
//   wfEcho(msg, { kind, sticky })
//   document.body.dispatchEvent(new CustomEvent('wfEcho', { detail: { msg, kind } }))
//
// Requires DOM elements: #mb-msg, #mb-time, #mb-history

(function () {
  "use strict";

  var msgEl     = document.getElementById("mb-msg");
  var timeEl    = document.getElementById("mb-time");
  var historyEl = document.getElementById("mb-history");
  if (!msgEl) return;

  var KINDS = new Set(["ok", "warn", "err", "info"]);
  var HISTORY_MAX = 8;
  var STALE_MS = 5000;
  var staleTimer = null;
  var history = [];

  function fmtTime(d) {
    d = d || new Date();
    var pad = function (n) { return String(n).padStart(2, "0"); };
    return pad(d.getHours()) + ":" + pad(d.getMinutes()) + ":" + pad(d.getSeconds());
  }

  function renderHistory() {
    if (!historyEl) return;
    while (historyEl.firstChild) historyEl.removeChild(historyEl.firstChild);
    if (!history.length) {
      var empty = document.createElement("div");
      empty.className = "row is-empty";
      var span = document.createElement("span");
      span.className = "msg";
      span.textContent = "No messages yet.";
      empty.appendChild(span);
      historyEl.appendChild(empty);
      return;
    }
    var items = history.slice(-HISTORY_MAX).reverse();
    for (var i = 0; i < items.length; i++) {
      var h = items[i];
      var row = document.createElement("div");
      row.className = "row" + (h.kind && KINDS.has(h.kind) ? " is-" + h.kind : "");
      var time = document.createElement("span");
      time.className = "time";
      time.textContent = h.time;
      var msg = document.createElement("span");
      msg.className = "msg";
      msg.textContent = h.msg;
      row.appendChild(time);
      row.appendChild(msg);
      historyEl.appendChild(row);
    }
  }

  function echo(msg, opts) {
    opts = opts || {};
    if (msg == null || msg === "") return;
    var kind = opts.kind && KINDS.has(opts.kind) ? opts.kind : "";

    msgEl.className = "wf-minibuffer-msg";
    void msgEl.offsetWidth;

    msgEl.textContent = msg;
    if (kind) msgEl.classList.add("is-" + kind);
    msgEl.classList.add("is-visible");

    var t = fmtTime();
    if (timeEl) timeEl.textContent = t;

    history.push({ msg: msg, kind: kind, time: t });
    if (history.length > 40) history.shift();
    renderHistory();

    clearTimeout(staleTimer);
    if (!opts.sticky) {
      staleTimer = setTimeout(function () { msgEl.classList.add("is-stale"); }, STALE_MS);
    }
  }

  function clearEcho() {
    clearTimeout(staleTimer);
    msgEl.classList.remove("is-visible", "is-stale", "is-ok", "is-warn", "is-err", "is-info");
    msgEl.textContent = "";
    if (timeEl) timeEl.textContent = "";
  }

  window.wfEcho = echo;
  window.wfEchoClear = clearEcho;

  document.body.addEventListener("wfEcho", function (e) {
    var d = e.detail || {};
    echo(d.msg || "", { kind: d.kind || "", sticky: d.sticky || false });
  });
})();
