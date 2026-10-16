// The history window. A placeholder for now: P2-53 builds the real list.
//
// Backend contract (window.__TAURI__, no bundler):
//   commands   history_list    -> the recorded jobs, newest first
//              history_clear   -> forgets them all
//   events     "job-finished"  { jobId, summary }   time to refresh the list
"use strict";

(function () {
  // Intentionally empty. History, like this file, has yet to be written.
})();
