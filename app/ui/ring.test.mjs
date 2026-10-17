// Tests for ring.math.js. Run with: node --test app/ui/ring.test.mjs
// No npm, no framework: node's own runner is enough for pure functions.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const M = require("./ring.math.js");
const { RING } = M;
const C = { x: 200, y: 200 };
const close = (a, b, eps = 1e-9) => assert.ok(Math.abs(a - b) < eps, `${a} != ${b}`);

// ───── geometry, mirroring crates/core/src/ring.rs ─────

test("span and mid angle start at noon and go clockwise", () => {
  close(M.span(0), M.TAU);
  close(M.span(4), Math.PI / 2);
  close(M.midAngle(0, 4), 0);
  close(M.midAngle(1, 4), Math.PI / 2);
});

test("point_at puts angle 0 straight up and pi/2 to the right", () => {
  const up = M.pointAt(C, 10, 0);
  close(up.x, 200);
  close(up.y, 190);
  const right = M.pointAt(C, 10, Math.PI / 2);
  close(right.x, 210);
  close(right.y, 200);
});

test("angle_of inverts point_at", () => {
  for (const a of [0, 0.3, 1.7, 3.1, 4.5, 6.2]) close(M.angleOf(C, M.pointAt(C, 50, a)), a, 1e-9);
});

test("index_at centres slice 0 on noon", () => {
  assert.equal(M.indexAt(0, 4), 0);
  assert.equal(M.indexAt(M.TAU - 0.1, 4), 0);
  assert.equal(M.indexAt(Math.PI / 4 - 0.01, 4), 0);
  assert.equal(M.indexAt(Math.PI / 4 + 0.01, 4), 1);
  assert.equal(M.indexAt(Math.PI, 4), 2);
  assert.equal(M.indexAt(1, 0), -1);
});

test("hit test: hole, ring, slack and outside", () => {
  assert.deepEqual(M.hitTest(C, { x: 200, y: 160 }, 63, 150, 6, 7), { kind: "center" });
  assert.deepEqual(M.hitTest(C, { x: 200, y: 100 }, 63, 150, 6, 7), { kind: "segment", index: 0 });
  assert.deepEqual(M.hitTest(C, { x: 300, y: 200 }, 63, 150, 4, 7), { kind: "segment", index: 1 });
  // 155 px out: past the edge, but inside the 7 px pop-out slack.
  assert.deepEqual(M.hitTest(C, { x: 200, y: 45 }, 63, 150, 6, 7), { kind: "segment", index: 0 });
  assert.deepEqual(M.hitTest(C, { x: 200, y: 45 }, 63, 150, 6, 0), { kind: "outside" });
  assert.deepEqual(M.hitTest(C, { x: 200, y: 100 }, 63, 150, 0, 7), { kind: "empty" });
});

test("num formats like ring.rs", () => {
  assert.equal(M.num(200), "200");
  assert.equal(M.num(1.5), "1.5");
  assert.equal(M.num(1.23456), "1.235");
  assert.equal(M.num(-0.0001), "0");
  assert.equal(M.num(-2.5), "-2.5");
});

test("a full turn draws the same ring path as ring.rs", () => {
  assert.equal(
    M.ringPath(C, 60, 150),
    "M 200 50 A 150 150 0 0 1 200 350 A 150 150 0 0 1 200 50 Z " +
      "M 200 140 A 60 60 0 0 1 200 260 A 60 60 0 0 1 200 140 Z",
  );
  assert.equal(M.slicePath(C, 60, 150, 0, 1), M.ringPath(C, 60, 150));
  assert.equal(M.slicePath(C, 60, 150, 0, 0), M.ringPath(C, 60, 150));
});

test("wedges keep a constant 3 px gap at both radii", () => {
  const n = 4;
  const path = M.slicePath(C, 60, 150, 0, n);
  const nums = path.match(/-?\d+(\.\d+)?/g).map(Number);
  // M x y A r r 0 large 1 x y L x y A r r 0 large 0 x y
  const [ox1, oy1, , , , large, sweep, ox2, oy2, ix1, iy1, , , , ilarge, isweep, ix2, iy2] = nums;
  assert.equal(large, 0);
  assert.equal(sweep, 1);
  assert.equal(ilarge, 0);
  assert.equal(isweep, 0);
  // Slice 0 is symmetric about the vertical through the centre.
  close(ox1 + ox2, 400, 0.002);
  close(oy1, oy2, 0.002);
  close(ix1 + ix2, 400, 0.002);
  // The gap to the neighbour is 3 px measured along the arc at each radius,
  // so each edge sits 1.5 px of arc short of the 45° boundary.
  const outerHalf = Math.atan2(ox2 - 200, 200 - oy2);
  const innerHalf = Math.atan2(ix1 - 200, 200 - iy1);
  close((Math.PI / 4 - outerHalf) * 150, 1.5, 0.01);
  close((Math.PI / 4 - innerHalf) * 60, 1.5, 0.01);
});

test("two slices use large arcs only when the wedge passes half a turn", () => {
  const two = M.slicePath(C, 60, 150, 0, 2);
  assert.match(two, / A 150 150 0 0 1 /);
  const big = M.wedgePath(C, 60, 150, 0, 4);
  assert.match(big, / A 150 150 0 1 1 /);
  assert.match(big, / A 60 60 0 1 0 /);
});

test("progress arc: nothing, a partial sweep, and a closed circle", () => {
  assert.equal(M.arcPath(C, 36, 0), "");
  assert.equal(M.arcPath(C, 36, 0.25), "M 200 164 A 36 36 0 0 1 236 200");
  assert.match(M.arcPath(C, 36, 0.75), /A 36 36 0 1 1 164 200$/);
  assert.equal(M.arcPath(C, 36, 1), "M 200 164 A 36 36 0 0 1 200 236 A 36 36 0 0 1 200 164");
});

test("inner radius clamps the hole ratio, and the side leaves room for shadow", () => {
  close(M.innerRadius(150, 0.42), 63);
  close(M.innerRadius(100, 0.05), 20);
  close(M.innerRadius(100, 0.9), 75);
  assert.equal(M.ringSide(150), 342);
});

// ───── motion ─────

test("easeOutBack starts at 0, overshoots, lands on 1", () => {
  close(M.easeOutBack(0), 0);
  close(M.easeOutBack(1), 1);
  const peak = Math.max(...Array.from({ length: 100 }, (_, i) => M.easeOutBack(i / 100)));
  assert.ok(peak > 1.05 && peak < 1.2, `peak ${peak}`);
});

test("open animation staggers slices and ends on time", () => {
  close(M.openAmount(0, 0), 0);
  close(M.openAmount(0, RING.openSeconds), 1);
  close(M.openAmount(3, 3 * RING.openStagger), 0);
  assert.ok(!M.openFinished(6, RING.openSeconds));
  assert.ok(M.openFinished(6, RING.openSeconds + 5 * RING.openStagger));
});

test("close animation shrinks every slice to nothing", () => {
  close(M.closeAmount(0, 0), 1);
  close(M.closeAmount(0, RING.closeSeconds), 0);
  assert.ok(M.closeFinished(4, RING.closeSeconds + 3 * RING.closeStagger));
});

test("slice transform: twist while opening, pop out along the mid angle on hover", () => {
  const opening = M.sliceTransform(0, 4, 0, 0);
  close(opening.rotate, -0.35);
  close(opening.scale, 0.55);
  close(opening.opacity, 0);
  const hovered = M.sliceTransform(1, 4, 1, 1);
  close(hovered.rotate, 0);
  close(hovered.scale, 1.025);
  close(hovered.dx, RING.popOut);
  close(hovered.dy, 0);
  // A spring dipping below zero must not pull the slice inwards.
  close(M.sliceTransform(0, 4, 1, -0.2).dy, 0);
});

function settle(spring, target, k, c, dt = 1 / 60) {
  let frames = 0;
  let peak = spring.value;
  while (M.stepSpring(spring, target, dt, k, c) && frames < 10_000) {
    frames++;
    peak = Math.max(peak, spring.value);
  }
  return { frames, peak };
}

test("springs settle and snap exactly onto the target", () => {
  const s = { value: 0, velocity: 0 };
  const { frames, peak } = settle(s, 1, 320, 20);
  assert.equal(s.value, 1);
  assert.equal(s.velocity, 0);
  assert.ok(frames < 120, `took ${frames} frames`);
  assert.ok(peak > 1, "hover-in should overshoot a little");
});

test("letting go of a hover never dips below zero", () => {
  const s = { value: 1, velocity: 0 };
  let low = 1;
  while (M.stepHover(s, false, 1 / 60)) low = Math.min(low, s.value);
  assert.ok(low >= -1e-6, `dipped to ${low}`);
  assert.equal(s.value, 0);
});

test("a late 50 ms frame does not blow the spring up", () => {
  // Without sub-stepping, k = 420 with a 50 ms step is unstable.
  const s = { value: 0, velocity: 0 };
  for (let i = 0; i < 40; i++) M.stepSpring(s, 1, 0.05, 420, 14);
  assert.ok(Math.abs(s.value - 1) < 0.05, `ended at ${s.value}`);
});

// ───── text ─────

const mono = (t) => t.length * 7; // 7 px per character

test("label width follows the arc length, clamped to 30..120", () => {
  close(M.labelWidth(M.span(1), 106.5), 120);
  close(M.labelWidth(M.span(12), 106.5), M.span(12) * 106.5 - 10);
  close(M.labelWidth(0.1, 50), 30);
});

test("caption hides when it only repeats the label", () => {
  assert.ok(M.showCaption("To PDF", "PDF"));
  assert.ok(!M.showCaption("PDF", "pdf"));
  assert.ok(!M.showCaption("PDF", ""));
});

test("wrapText fits words, caps lines and ellipsizes the rest", () => {
  assert.deepEqual(M.wrapText("To PDF", 100, 2, mono), ["To PDF"]);
  assert.deepEqual(M.wrapText("one two three", 50, 2, mono), ["one two", "three"]);
  const capped = M.wrapText("one two three four five", 50, 2, mono);
  assert.equal(capped.length, 2);
  assert.ok(capped[1].endsWith("…"), capped[1]);
  assert.ok(capped.every((l) => mono(l) <= 50), JSON.stringify(capped));
  const long = M.wrapText("Supercalifragilistic", 50, 1, mono);
  assert.equal(long.length, 1);
  assert.ok(long[0].endsWith("…") && mono(long[0]) <= 50, long[0]);
  assert.deepEqual(M.wrapText("", 50, 2, mono), []);
});

test("hub hints follow Filee's click-mode wording", () => {
  assert.equal(M.hintFor(null), "Esc to close");
  assert.equal(M.hintFor({ label: "PNG", enabled: true }), "Click → PNG");
  assert.equal(
    M.hintFor({ label: "PDF", enabled: false, disabledReason: "Needs LibreOffice" }),
    "Needs LibreOffice",
  );
});

// ───── progress ring ─────

function run(ring, seconds, dt = 1 / 60) {
  let t = 0;
  while (t < seconds) {
    ring.advance(dt);
    t += dt;
  }
}

test("progress: slices collapse, then the ring pops in", () => {
  const ring = new M.ProgressRing(4);
  assert.equal(ring.collapseAll(), 0);
  run(ring, 0.2);
  assert.ok(ring.collapse(0) > ring.collapse(3), "slices collapse with a stagger");
  assert.equal(ring.ringClock, -1);
  run(ring, 0.3);
  assert.equal(ring.collapseAll(), 1);
  assert.ok(ring.ringClock >= 0);
  run(ring, 1);
  close(ring.pop.value, 1, 0.01);
});

test("progress: the arc chases the real progress", () => {
  const ring = new M.ProgressRing(3);
  ring.setProgress(0.5);
  run(ring, 1.5);
  close(ring.shown, 0.5);
  ring.setProgress(7);
  run(ring, 1.5);
  close(ring.shown, 1);
});

test("progress: success waits for the minimum ring time, holds, fades, finishes", () => {
  const ring = new M.ProgressRing(2);
  ring.complete("succeeded");
  run(ring, 0.3);
  assert.equal(ring.outcome, null, "too early for the badge");
  run(ring, 1.0);
  assert.equal(ring.outcome, "succeeded");
  close(ring.shown, 1);
  run(ring, RING.holdSeconds.succeeded + RING.fadeSeconds + 0.1);
  assert.equal(ring.phase, "done");
  assert.equal(ring.advance(1 / 60), false);
});

test("progress: a failure keeps a red sliver and stays up for the message", () => {
  const ring = new M.ProgressRing(2);
  ring.complete("failed");
  run(ring, 3);
  assert.equal(ring.outcome, "failed");
  assert.equal(ring.phase, "failed");
  close(ring.arcFraction(), 0.02);
  assert.equal(ring.advance(1 / 60), false);
});

test("progress: without keepFailure a failure fades like Filee's", () => {
  const ring = new M.ProgressRing(2, { keepFailure: false });
  ring.complete("failed");
  run(ring, 0.9 + RING.holdSeconds.failed + RING.fadeSeconds + 0.5);
  assert.equal(ring.phase, "done");
});

test("progress: the first outcome wins and later progress is ignored", () => {
  const ring = new M.ProgressRing(1);
  ring.complete("cancelled");
  ring.complete("succeeded");
  ring.setProgress(0.9);
  run(ring, 2);
  assert.equal(ring.outcome, "cancelled");
  assert.equal(ring.phase, "done");
});

test("progress: reduced motion snaps the animations but keeps the holds", () => {
  const ring = new M.ProgressRing(5, { reduced: true });
  assert.equal(ring.collapse(4), 1);
  assert.equal(ring.pop.value, 1);
  ring.setProgress(0.4);
  ring.advance(1 / 60);
  assert.equal(ring.shown, 0.4);
  ring.complete("succeeded");
  run(ring, 0.5);
  assert.equal(ring.outcome, null, "the ring still stays up for 0.6 s");
  run(ring, 0.2);
  assert.equal(ring.outcome, "succeeded");
  assert.equal(ring.badge.value, 1);
  run(ring, RING.holdSeconds.succeeded - 0.3);
  assert.equal(ring.phase, "running", "the ✓ is held, not skipped");
  run(ring, 0.4);
  assert.equal(ring.phase, "done");
});

// ───── frame-loop watchdog ─────

test("watchdog: frames arriving in time just keep it watching", () => {
  const { stalledMs } = M.WATCHDOG;
  assert.deepEqual(M.watchdogCheck(1000 + stalledMs - 1, 1000, 0), { action: "watch", stalls: 0 });
  assert.deepEqual(M.watchdogCheck(1000, 1000, 1), { action: "watch", stalls: 1 });
});

test("watchdog: a stalled loop is retried twice, then finished", () => {
  assert.equal(M.WATCHDOG.stalledMs, 250, "Filee's StalledLoopMs");
  assert.equal(M.WATCHDOG.retries, 2, "Filee's FrameRetries");
  // A fake clock and a frame loop that never answers.
  let now = 0;
  const lastFrameAt = 0;
  let stalls = 0;
  const actions = [];
  for (let i = 0; i < 3; i++) {
    now += M.WATCHDOG.checkMs;
    const check = M.watchdogCheck(now, lastFrameAt, stalls);
    stalls = check.stalls;
    actions.push(check.action);
  }
  assert.deepEqual(actions, ["retry", "retry", "finish"]);
  assert.equal(stalls, 3);
});

test("watchdog: stalls count per animation, so trickling frames still finish", () => {
  // One frame sneaks in after every retry: the count must not reset, or the
  // donut would open one frame per 300 ms forever.
  let now = 0;
  let lastFrameAt = 0;
  let stalls = 0;
  const actions = [];
  for (let i = 0; i < 3; i++) {
    now += M.WATCHDOG.checkMs;
    const check = M.watchdogCheck(now, lastFrameAt, stalls);
    stalls = check.stalls;
    actions.push(check.action);
    lastFrameAt = now - M.WATCHDOG.stalledMs - 1; // a frame long ago again
  }
  assert.deepEqual(actions, ["retry", "retry", "finish"]);
  assert.equal(M.watchdogCheck(300, 0, 0, 0).action, "finish", "zero retries finishes at once");
});

test("finishAll: an opening menu ends fully open with the hover settled", () => {
  const hover = [
    { value: 0.3, velocity: 4 },
    { value: 0.6, velocity: -2 },
    { value: 0, velocity: 1 },
  ];
  const state = { mode: "menu", clock: 0.01, hover, hoverIndex: 1, progress: null };
  assert.equal(M.finishAll(state), false);
  assert.ok(M.openFinished(hover.length, state.clock));
  for (let i = 0; i < 3; i++) close(M.openAmount(i, state.clock), 1);
  assert.deepEqual(
    hover.map((s) => [s.value, s.velocity]),
    [
      [0, 0],
      [1, 0],
      [0, 0],
    ],
  );
});

test("finishAll: a closing ring asks to hide", () => {
  const state = { mode: "closing", clock: 0, hover: [], hoverIndex: -1, progress: null };
  assert.equal(M.finishAll(state), true);
  assert.ok(M.closeFinished(8, state.clock));
});

test("finishAll: the progress ring snaps its looks but keeps its timers", () => {
  const ring = new M.ProgressRing(6);
  ring.setProgress(0.4);
  ring.complete("succeeded");
  const state = { mode: "progress", clock: 0, hover: [], hoverIndex: -1, progress: ring };
  assert.equal(M.finishAll(state), false);
  assert.equal(ring.collapse(5), 1);
  assert.equal(ring.ringClock, 0);
  assert.equal(ring.pop.value, 1);
  assert.equal(ring.shown, ring.target);
  assert.equal(ring.outcome, null, "the minimum ring time still applies");
  ring.advance(RING.minRingSeconds + 0.01);
  assert.equal(ring.outcome, "succeeded");
  M.finishAll(state);
  assert.equal(ring.badge.value, 1);
  assert.equal(ring.phase, "running", "the ✓ hold is not skipped");
});

test("finishAll: a hidden ring is left alone", () => {
  const state = { mode: "hidden", clock: 0.2, hover: [], hoverIndex: -1, progress: null };
  assert.equal(M.finishAll(state), false);
  assert.equal(state.clock, 0.2);
});

// ───── the cancel button on the progress ring (DonutMenu.Progress.cs) ─────

test("cancel: the button sits on the ring's outer edge at 45 degrees", () => {
  const b = M.cancelButtonCenter(C);
  const r = RING.ringRadius + RING.ringStroke;
  close(b.x, 200 + r * Math.SQRT1_2);
  close(b.y, 200 - r * Math.SQRT1_2);
  close(RING.cancelFadeSeconds, 0.12);
});

test("cancel: progress hit test table", () => {
  const b = M.cancelButtonCenter(C);
  const plate = RING.ringRadius + RING.ringStroke + 2;
  // [point, cancelShown, expected kind, why]
  const rows = [
    [b, true, "cancel", "dead centre of the ×"],
    [{ x: b.x + RING.cancelHitRadius - 0.01, y: b.y }, true, "cancel", "just inside the × hit radius"],
    [{ x: b.x, y: b.y - RING.cancelHitRadius - 0.5 }, true, "outside", "just past the ×, off the plate"],
    [b, false, "ring", "a hidden × is not a button; its centre is on the plate"],
    [C, true, "ring", "the middle of the ring"],
    [C, false, "ring", "the middle of the ring, × hidden"],
    [{ x: C.x - plate + 0.5, y: C.y }, true, "ring", "the plate's far edge"],
    [{ x: C.x - plate - 0.5, y: C.y }, true, "outside", "just off the plate"],
    [{ x: C.x, y: C.y + 120 }, true, "outside", "well away"],
  ];
  for (const [p, shown, kind, why] of rows) {
    assert.equal(M.progressHitTest(C, p, shown).kind, kind, why);
  }
});

test("cancel: hovering reaches 14 px past the ring's outer edge", () => {
  const edge = RING.ringRadius + RING.ringStroke + RING.cancelHoverSlack;
  assert.equal(M.progressHover(C, C), true);
  assert.equal(M.progressHover(C, { x: C.x + edge, y: C.y }), true);
  assert.equal(M.progressHover(C, { x: C.x + edge + 0.5, y: C.y }), false);
  // The whole × is inside the hover zone, so reaching for it never hides it.
  const b = M.cancelButtonCenter(C);
  const reach = RING.cancelHitRadius * Math.SQRT1_2;
  assert.equal(M.progressHover(C, { x: b.x + reach, y: b.y - reach }), true);
});

test("cancel: the × shows only while hovering a running, visible ring", () => {
  // [hovering, ringVisible, running, cancelWanted, shown]
  const rows = [
    [true, true, true, false, true],
    [false, true, true, false, false],
    [true, false, true, false, false],
    [true, true, false, false, false],
    [true, true, true, true, false],
  ];
  for (const [h, v, r, w, want] of rows) {
    assert.equal(M.showCancel(h, v, r, w), want, JSON.stringify([h, v, r, w]));
  }
});

// ───── clicking the finished ring (RadialController's ProgressClicked) ─────

test("click: the finished ring reveals the output, the running one does not", () => {
  const running = { running: true, finished: false, failedHeld: false, hasOutput: false };
  const done = { running: false, finished: true, failedHeld: false, hasOutput: true };
  assert.equal(M.progressClickAction("ring", done), "reveal");
  assert.equal(M.progressClickAction("ring", running), "none");
  // Even a running job that somehow knows an output keeps its plate inert.
  assert.equal(M.progressClickAction("ring", { ...running, hasOutput: true }), "none");
  assert.equal(M.progressClickAction("cancel", running), "cancel");
  assert.equal(M.progressClickAction("cancel", done), "none");
  assert.equal(M.progressClickAction("outside", done), "none");
});

test("click: nothing to reveal without an output", () => {
  const cancelled = { running: false, finished: true, failedHeld: false, hasOutput: false };
  assert.equal(M.progressClickAction("ring", cancelled), "none");
  const failed = { ...cancelled, failedHeld: true };
  assert.equal(M.progressClickAction("ring", failed), "close");
  assert.equal(M.progressClickAction("outside", failed), "close");
  // A partly failed job wrote something, so its plate still shows it.
  assert.equal(M.progressClickAction("ring", { ...failed, hasOutput: true }), "reveal");
});

test("click: the state machine's done hold is when a click can reveal", () => {
  const ring = new M.ProgressRing(3, { reduced: true });
  const state = () => ({
    running: !ring.pending,
    finished: !!ring.pending,
    failedHeld: ring.phase === "failed",
    hasOutput: true,
  });
  ring.advance(0.016);
  ring.setProgress(0.5);
  ring.advance(0.016);
  assert.equal(M.progressClickAction("ring", state()), "none");
  ring.complete("succeeded");
  ring.advance(RING.minRingSeconds + 0.01);
  assert.equal(ring.outcome, "succeeded");
  assert.notEqual(ring.phase, "done");
  assert.equal(M.progressClickAction("ring", state()), "reveal");
});

// ───── hand-off to the toast (RadialController's HandOffToToast) ─────

const runningJob = {
  running: true,
  outcome: null,
  jobKnown: true,
  handedOff: false,
  cancelWanted: false,
  failedHeld: false,
};

test("hand-off: a job still running after 5 s moves to the toast", () => {
  assert.equal(RING.handOffSeconds, 5);
  assert.equal(M.handOffAction(0, runningJob, null), "none");
  assert.equal(M.handOffAction(4.99, runningJob, null), "none");
  assert.equal(M.handOffAction(5, runningJob, null), "hand-off");
  assert.equal(M.handOffAction(42, runningJob, null), "hand-off");
});

test("hand-off: once is enough, and only with a job id to hand over", () => {
  assert.equal(M.handOffAction(6, { ...runningJob, handedOff: true }, null), "none");
  assert.equal(M.handOffAction(6, { ...runningJob, jobKnown: false }, null), "none");
  // A job being cancelled ends by itself; it gets no card.
  assert.equal(M.handOffAction(6, { ...runningJob, cancelWanted: true }, null), "none");
});

test("hand-off: a failure goes to the toast at once, whenever it happens", () => {
  const failed = { ...runningJob, running: false, outcome: "failed" };
  assert.equal(M.handOffAction(0.2, failed, null), "report");
  assert.equal(M.handOffAction(9, failed, null), "report");
  assert.equal(M.handOffAction(0.2, { ...failed, handedOff: true }, null), "none");
  // Never queued, so no job for the toast: the ring keeps the failure itself.
  assert.equal(M.handOffAction(0.2, { ...failed, jobKnown: false }, null), "none");
  // Good news and cancels stay on the ring.
  for (const outcome of ["succeeded", "cancelled"]) {
    assert.equal(M.handOffAction(9, { ...runningJob, running: false, outcome }, null), "none");
  }
});

test("hand-off: Escape while running hands off instead of cancelling", () => {
  assert.equal(M.handOffAction(0.1, runningJob, "Escape"), "hand-off");
  assert.equal(M.handOffAction(0.1, runningJob, "dismiss"), "hand-off");
  // Nothing to hand over, or already stopping: Escape just closes.
  assert.equal(M.handOffAction(0.1, { ...runningJob, jobKnown: false }, "Escape"), "close");
  assert.equal(M.handOffAction(0.1, { ...runningJob, cancelWanted: true }, "Escape"), "close");
  const finishing = { ...runningJob, running: false, outcome: "succeeded" };
  assert.equal(M.handOffAction(0.1, finishing, "Escape"), "close");
  // No key ever means "cancel": that stays on the × and the toast.
  for (const key of [null, "Escape", "Enter", "dismiss", "x"]) {
    assert.notEqual(M.handOffAction(10, runningJob, key), "cancel");
  }
});

test("hand-off: Enter closes a held failure and nothing else", () => {
  const held = { ...runningJob, running: false, outcome: "failed", jobKnown: false, failedHeld: true };
  assert.equal(M.handOffAction(3, held, "Enter"), "close");
  assert.equal(M.handOffAction(3, runningJob, "Enter"), "none");
  assert.equal(M.handOffAction(10, runningJob, "a"), "none");
});
