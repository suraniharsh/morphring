// The drop zone. No bundler, no framework: Tauri injects window.__TAURI__ because
// withGlobalTauri is on, and that is all the API this page needs.
"use strict";

(function () {
  const zone = document.getElementById("drop-zone");
  const dropped = document.getElementById("dropped");
  const title = document.getElementById("dropped-title");
  const note = document.getElementById("dropped-note");
  const list = document.getElementById("dropped-list");

  const tauri = window.__TAURI__;
  if (!tauri) {
    // Opened in a plain browser (handy for tweaking CSS). HTML5 drag and drop
    // can't give us real paths anyway, so don't pretend.
    document.getElementById("no-tauri").hidden = false;
    return;
  }

  // Hover feedback comes straight from Tauri's drag-drop events. The paths
  // themselves are filtered in Rust (dropzone.rs) and arrive as "files-dropped".
  tauri.webview.getCurrentWebview().onDragDropEvent((event) => {
    const type = event.payload.type;
    zone.classList.toggle("over", type === "enter" || type === "over");
  });

  tauri.event.listen("files-dropped", (event) => show(event.payload));

  function show(payload) {
    const files = payload.files || [];
    const ignored = payload.ignored || 0;

    title.textContent =
      files.length === 0
        ? "Nothing to convert"
        : files.length === 1
          ? "1 file ready"
          : files.length + " files ready";
    note.textContent =
      ignored === 0
        ? ""
        : ignored === 1
          ? "1 folder or missing item was left out."
          : ignored + " folders or missing items were left out.";

    // textContent only: file names are user data, and "<script>.png" is a
    // perfectly legal file name on Linux.
    list.replaceChildren(
      ...files.map((path) => {
        const item = document.createElement("li");
        item.textContent = baseName(path);
        item.title = path;
        return item;
      }),
    );
    dropped.hidden = false;
  }

  function baseName(path) {
    const parts = path.split(/[\\/]/);
    return parts[parts.length - 1] || path;
  }
})();
