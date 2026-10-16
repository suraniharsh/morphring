// The settings window. A placeholder for now: P2-52 builds the real page.
//
// Backend contract (window.__TAURI__, no bundler):
//   commands   settings_get              -> the app settings (camelCase JSON)
//              settings_set { settings } -> validates and saves them
//              presets_list              -> [{ id, name, target }]
"use strict";

(function () {
  // Nothing to wire yet. The page is a heading and a promise.
})();
