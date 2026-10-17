// The toast window: one card per job that outlives the ring.
//
// Backend contract (window.__TAURI__, no bundler). The toast is driven only by
// these app-wide events, which every job sends whether the ring shows it or not:
//   "job-started"   { jobId: number, files: string[], label: string }
//                   First event of a job. `files` are the source paths, `label`
//                   is what it converts to, e.g. "JPG".
//   "job-progress"  { jobId: number, progress: 0..1 }
//                   Sent when the job's progress moved by at least 1 %; it only
//                   goes up. The last one of a finished job is exactly 1.
//   "job-finished"  { jobId: number,
//                     summary: { outcome: "succeeded" | "failed" | "cancelled",
//                                outputs: string[], done: number, failed: number,
//                                message?: string } }
//                   Last event of a job, whatever the outcome.
//   "toast-dismiss" jobId           drop that job's card (sent by toast_dismiss)
//   "toast-show"    jobId           show that job's card now, running or
//                                   finished (sent when the ring hands it off)
// Job ids start at 1 and go up by one per job. Jobs run one at a time, so the
// events of two jobs never interleave.
//   commands the toast invokes
//     toast_check { runningMs }       -> ms until the card may show (0 = now),
//                                        null while the ring is on screen
//     toast_dismiss_delay { outcome } -> ms a finished card stays
//     toast_layout { height }         sizes, places and shows the window (0 hides)
//     cancel_job { jobId }            stops a running job
//     toast_dismiss { jobId }         drops a card
//     opener.revealItemInDir(path)    "Show"
"use strict";

(function () {
  const tauri = window.__TAURI__;
  const list = document.getElementById("cards");

  // How often a waiting card asks whether it may show. Short enough that
  // closing the ring feels answered, long enough not to chatter.
  const POLL_MS = 250;

  // How long a job that ended without a card is remembered, in case the ring
  // hands it over a beat after job-finished (a failure always is).
  const HAND_OFF_GRACE_MS = 3000;

  /** jobId -> { job, el, parts, startedAt, shown, finished, summary, timer } */
  const jobs = new Map();

  function invoke(cmd, args) {
    if (!tauri) return Promise.reject(new Error("no Tauri backend"));
    return tauri.core.invoke(cmd, args);
  }

  function fileName(path) {
    const parts = String(path).split(/[\\/]/);
    return parts[parts.length - 1] || String(path);
  }

  function title(started) {
    const files = started.files || [];
    const what = files.length === 1 ? fileName(files[0]) : files.length + " files";
    return what + " → " + started.label;
  }

  function statusText(summary) {
    switch (summary.outcome) {
      case "succeeded":
        return "Done";
      case "cancelled":
        return "Cancelled";
      default: {
        const total = summary.done + summary.failed;
        if (summary.done > 0 && summary.failed > 0) {
          return summary.failed + " of " + total + " failed";
        }
        return summary.message || "Conversion failed";
      }
    }
  }

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function button(text, onClick) {
    const node = el("button", "", text);
    node.type = "button";
    node.addEventListener("click", onClick);
    return node;
  }

  function buildCard(entry) {
    const card = el("li", "card");
    const name = el("p", "title", title(entry.job));
    name.title = (entry.job.files || []).join("\n");
    const row = el("div", "row");
    const bar = el("progress", "bar");
    bar.max = 1;
    bar.value = 0;
    const percent = el("span", "percent", "0 %");
    row.append(bar, percent);
    const status = el("p", "status", "Converting…");
    status.setAttribute("role", "status");
    const actions = el("div", "actions");
    const cancel = button("Cancel", function () {
      cancel.disabled = true;
      status.textContent = "Cancelling…";
      invoke("cancel_job", { jobId: entry.job.jobId }).catch(function (e) {
        status.textContent = "Could not cancel: " + e;
        cancel.disabled = false;
      });
    });
    const show = button("Show", function () {
      const first = entry.outputs && entry.outputs[0];
      if (!first || !tauri || !tauri.opener) return;
      tauri.opener.revealItemInDir(first).catch(function (e) {
        status.textContent = "Could not show the file: " + e;
      });
    });
    show.hidden = true;
    const dismiss = button("Dismiss", function () {
      // Through the backend, so every way to dismiss takes the same road.
      invoke("toast_dismiss", { jobId: entry.job.jobId }).catch(function () {
        remove(entry.job.jobId);
      });
    });
    actions.append(cancel, show, dismiss);
    card.append(name, row, status, actions);
    entry.el = card;
    entry.parts = { bar: bar, percent: percent, status: status, cancel: cancel, show: show };
  }

  // Tell the backend how tall the cards are; 0 hides the window. Waits a frame
  // so the browser has laid the new cards out before we measure them.
  let layoutQueued = false;
  function relayout() {
    if (layoutQueued) return;
    layoutQueued = true;
    requestAnimationFrame(function () {
      layoutQueued = false;
      const height = list.children.length ? Math.ceil(document.body.scrollHeight) : 0;
      invoke("toast_layout", { height: height }).catch(function (e) {
        console.warn("toast layout failed:", e);
      });
    });
  }

  function remove(jobId) {
    const entry = jobs.get(jobId);
    if (!entry) return;
    clearTimeout(entry.timer);
    jobs.delete(jobId);
    if (entry.el && entry.el.parentNode) {
      entry.el.remove();
      relayout();
    }
  }

  function reveal(entry) {
    entry.shown = true;
    list.append(entry.el);
    relayout();
  }

  // Asks the backend, every POLL_MS, whether this job has earned its card.
  function poll(jobId) {
    const entry = jobs.get(jobId);
    if (!entry || entry.shown || entry.finished) return;
    const runningMs = Math.max(0, Math.round(performance.now() - entry.startedAt));
    invoke("toast_check", { runningMs: runningMs })
      .then(function (left) {
        const now = jobs.get(jobId);
        if (!now || now.shown || now.finished) return;
        if (left === 0) {
          reveal(now);
        } else {
          now.timer = setTimeout(poll, Math.min(left == null ? POLL_MS : left, POLL_MS), jobId);
        }
      })
      .catch(function (e) {
        console.warn("toast check failed:", e);
      });
  }

  function onStarted(job) {
    const entry = { job: job, startedAt: performance.now(), shown: false, finished: false };
    buildCard(entry);
    jobs.set(job.jobId, entry);
    poll(job.jobId);
  }

  function onProgress(p) {
    const entry = jobs.get(p.jobId);
    if (!entry || entry.finished) return;
    const value = Math.max(0, Math.min(1, p.progress));
    entry.parts.bar.value = value;
    entry.parts.percent.textContent = Math.floor(value * 100) + " %";
  }

  function onFinished(f) {
    const entry = jobs.get(f.jobId);
    if (!entry) return;
    entry.finished = true;
    clearTimeout(entry.timer);
    entry.summary = f;
    // A job that ended before it earned a card leaves without a word, unless
    // the ring hands it over in the next moment.
    if (!entry.shown) {
      entry.timer = setTimeout(remove, HAND_OFF_GRACE_MS, f.jobId);
      return;
    }
    showFinished(entry, f);
  }

  function showFinished(entry, f) {
    const summary = f.summary;
    entry.outputs = summary.outputs || [];
    entry.el.classList.add("finished", summary.outcome);
    if (summary.outcome === "succeeded") {
      entry.parts.bar.value = 1;
      entry.parts.percent.textContent = "100 %";
    }
    entry.parts.status.textContent = statusText(summary);
    entry.parts.cancel.hidden = true;
    entry.parts.show.hidden = entry.outputs.length === 0;
    relayout();
    invoke("toast_dismiss_delay", { outcome: summary.outcome })
      .catch(function () {
        return 15000;
      })
      .then(function (ms) {
        if (jobs.get(f.jobId) === entry) entry.timer = setTimeout(remove, ms, f.jobId);
      });
  }

  // The ring handed this job over: no waiting, card up now.
  function onShow(jobId) {
    const entry = jobs.get(jobId);
    if (!entry || entry.shown) return;
    clearTimeout(entry.timer);
    reveal(entry);
    if (entry.finished && entry.summary) showFinished(entry, entry.summary);
  }

  function listen(name, handler) {
    tauri.event.listen(name, function (event) {
      handler(event.payload);
    }).catch(function (e) {
      console.warn("could not listen for " + name + ":", e);
    });
  }

  if (!tauri) return;
  listen("job-started", onStarted);
  listen("job-progress", onProgress);
  listen("job-finished", onFinished);
  listen("toast-dismiss", remove);
  listen("toast-show", onShow);
})();
