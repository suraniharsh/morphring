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
