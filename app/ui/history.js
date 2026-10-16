// The history window: every finished job, newest first.
//
// Backend contract (window.__TAURI__, no bundler):
//   commands   history_list    -> the recorded jobs, newest first
//              history_clear   -> forgets them all
//   events     "job-finished"    { jobId, summary }   time to refresh the list
//              "history-changed" (no payload)        history.json was rewritten
"use strict";

(function () {
  const tauri = window.__TAURI__;
  const list = document.getElementById("history");
  const empty = document.getElementById("empty");
  const error = document.getElementById("error");
  const clearButton = document.getElementById("clear");

  const STATE_TEXT = {
    Completed: "Completed",
    CompletedWithErrors: "Completed with errors",
    Failed: "Failed",
    Cancelled: "Cancelled",
  };

  function plural(n, word) {
    return n + " " + word + (n === 1 ? "" : "s");
  }

  function fileName(path) {
    const parts = String(path).split(/[\\/]/);
    return parts[parts.length - 1] || String(path);
  }

  function formatTime(raw) {
    const at = new Date(raw);
    // The core writes the epoch for dates it could not read; showing 1970
    // would be technically honest and practically useless.
    if (isNaN(at.getTime()) || at.getTime() === 0) return "";
    return at.toLocaleString();
  }

  function reveal(path) {
    if (!tauri || !tauri.opener) return;
    tauri.opener.revealItemInDir(path).catch(function (e) {
      showError("Could not show the file: " + e);
    });
  }

  function showError(text) {
    error.textContent = text;
    error.hidden = !text;
  }

  function el(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }

  function renderEntry(entry) {
    const item = el("li", "job");
    const head = el("div", "job-head");
    head.append(el("span", "job-label", entry.presetName || entry.presetId || "Conversion"));
    const state = entry.state || "Failed";
    head.append(el("span", "job-state " + state.toLowerCase(), STATE_TEXT[state] || state));
    const sources = (entry.sources || []).length;
    const outputs = entry.outputs || [];
    const failures = entry.failures || [];
    let counts = plural(sources, "file") + ", " + outputs.length + " converted";
    if (failures.length) counts += ", " + failures.length + " failed";
    head.append(el("span", "job-counts", counts));
    const time = formatTime(entry.finishedAt);
    if (time) {
      const t = el("time", "job-time", time);
      t.dateTime = entry.finishedAt;
      head.append(t);
    }
    item.append(head);

    if (outputs.length) {
      const outs = el("ul", "job-outputs");
      outputs.forEach(function (path) {
        const li = el("li");
        const button = el("button", "", fileName(path));
        button.type = "button";
        button.title = "Show " + path + " in its folder";
        button.addEventListener("click", function () {
          reveal(path);
        });
        li.append(button);
        outs.append(li);
      });
      item.append(outs);
    }

    if (failures.length) {
      const fails = el("ul", "job-failures");
      failures.forEach(function (f) {
        const why = f.errorDetail || f.errorKey || "failed";
        fails.append(el("li", "", fileName(f.source) + ": " + why));
      });
      item.append(fails);
    }
    return item;
  }

  function render(entries) {
    list.replaceChildren.apply(list, entries.map(renderEntry));
    empty.hidden = entries.length > 0;
    clearButton.disabled = entries.length === 0;
  }

  async function refresh() {
    if (!tauri) return;
    try {
      const entries = await tauri.core.invoke("history_list");
      showError("");
      render(Array.isArray(entries) ? entries : []);
    } catch (e) {
      showError("Could not load the history: " + e);
    }
  }

  clearButton.addEventListener("click", async function () {
    if (!tauri) return;
    clearButton.disabled = true;
    try {
      await tauri.core.invoke("history_clear");
      showError("");
      render([]);
    } catch (e) {
      showError(String(e));
      clearButton.disabled = false;
    }
  });

  if (tauri) {
    // job-finished goes out a moment before the entry hits the disk, so give
    // the backend a beat; history-changed then confirms with the real thing.
    tauri.event.listen("job-finished", function () {
      setTimeout(refresh, 250);
    });
    tauri.event.listen("history-changed", refresh);
    // The window is hidden, not closed, so refresh whenever it comes back.
    window.addEventListener("focus", refresh);
  }
  refresh();
})();
