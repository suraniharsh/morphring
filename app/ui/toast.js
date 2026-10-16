// The toast window: one card per long-running job. A placeholder for now;
// P2-54 builds the cards.
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
// Job ids start at 1 and go up by one per job. Jobs run one at a time, so the
// events of two jobs never interleave.
//   commands the toast invokes
//     cancel_job { jobId }      stops a running job
//     toast_dismiss { jobId }   drops a card
"use strict";

(function () {
  // Listening starts with the real toast. Until then: silence, beautifully styled.
})();
