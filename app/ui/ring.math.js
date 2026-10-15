// The ring's pure half: geometry, springs, easing, text wrapping and the
// progress state machine. No DOM in here, so node can test it (ring.test.mjs)
// and ring.js only has to turn numbers into SVG attributes.
//
// The geometry mirrors crates/core/src/ring.rs line for line, number
// formatting included, so a path drawn here and a path asserted in Rust are
// the same string. Ported from Filee's DonutGeometry.cs, DonutMenu.cs and
// DonutMenu.Progress.cs.
"use strict";

(function (root, factory) {
  if (typeof module === "object" && module.exports) {
    module.exports = factory();
  } else {
    root.RingMath = factory();
  }
})(typeof self !== "undefined" ? self : this, function () {
  const TAU = Math.PI * 2;

  /** Shape and timing constants, straight from Filee's DonutMenu. */
  const RING = Object.freeze({
    outerRadius: 150,
    holeRatio: 0.42,
    sliceOpacity: 0.96,
    gapPx: 3,
    popOut: 7,
    // Room for the soft shadow around the popped-out slices.
    margin: 14,
    openSeconds: 0.42,
    openStagger: 0.028,
    closeSeconds: 0.16,
    closeStagger: 0.01,
    ringRadius: 36,
    ringStroke: 6,
    collapseSeconds: 0.28,
    collapseStagger: 0.02,
    minRingSeconds: 0.6,
    holdSeconds: Object.freeze({ succeeded: 0.9, failed: 1.4, cancelled: 0.2 }),
    fadeSeconds: 0.25,
  });

  // Below this a "full turn" is a full turn. Same epsilons as ring.rs.
  const FULL_TURN_EPSILON = 0.001;
  const MIN_HALF_ANGLE = 0.001;

  function clamp(v, lo, hi) {
    return v < lo ? lo : v > hi ? hi : v;
  }

  function span(count) {
    return count <= 0 ? TAU : TAU / count;
  }

  function midAngle(index, count) {
    return index * span(count);
  }

  // Angles run clockwise from 12 o'clock, and screen y grows downwards, hence
  // the minus on cos.
  function pointAt(c, r, a) {
    return { x: c.x + r * Math.sin(a), y: c.y - r * Math.cos(a) };
  }

  function angleOf(c, p) {
    const a = Math.atan2(p.x - c.x, c.y - p.y);
    return a < 0 ? a + TAU : a;
  }

  /** Index of the slice under angle `a`, or -1 when there are no slices. */
  function indexAt(a, count) {
    if (count <= 0) return -1;
    const s = span(count);
    // Slice 0 is centred on noon, so shift by half a slice before flooring.
    return ((Math.floor((a + s / 2) / s) % count) + count) % count;
  }

  /**
   * What sits under point `p`: `{ kind: "center" | "outside" | "empty" }` or
   * `{ kind: "segment", index }`. `slack` widens the outer edge so a slice that
   * popped out on hover is still under the pointer.
   */
  function hitTest(c, p, innerRadius, outerRadius, count, slack) {
    const d = Math.hypot(p.x - c.x, p.y - c.y);
    if (d < innerRadius) return { kind: "center" };
    if (d > outerRadius + (slack || 0)) return { kind: "outside" };
    if (count <= 0) return { kind: "empty" };
    return { kind: "segment", index: indexAt(angleOf(c, p), count) };
  }

  // Three decimals, no trailing zeros, no "-0". Matches num() in ring.rs, so
  // the two sides can be diffed by eye.
  function num(v) {
    let s = v.toFixed(3);
    if (s.includes(".")) s = s.replace(/0+$/, "").replace(/\.$/, "");
    return s === "-0" ? "0" : s;
  }

  function ringPath(c, innerRadius, outerRadius) {
    // A single arc can't close a circle (start == end), so it's two halves.
    const circle = (r) => {
      const x = num(c.x);
      const top = num(c.y - r);
      const bottom = num(c.y + r);
      const rr = num(r);
      return `M ${x} ${top} A ${rr} ${rr} 0 0 1 ${x} ${bottom} A ${rr} ${rr} 0 0 1 ${x} ${top} Z`;
    };
    return `${circle(outerRadius)} ${circle(innerRadius)}`;
  }

  /**
   * SVG path of a wedge of `sliceSpan` radians centred on `mid`. A full turn
   * gives a full ring, which needs `fill-rule="evenodd"`.
   */
  function wedgePath(c, innerRadius, outerRadius, mid, sliceSpan) {
    if (sliceSpan >= TAU - FULL_TURN_EPSILON) return ringPath(c, innerRadius, outerRadius);
    // Constant gap in pixels, not in angle: each radius trims its own half-angle.
    const outerHalf = Math.max(MIN_HALF_ANGLE, sliceSpan / 2 - RING.gapPx / 2 / outerRadius);
    const innerHalf = Math.max(MIN_HALF_ANGLE, sliceSpan / 2 - RING.gapPx / 2 / innerRadius);
    const o1 = pointAt(c, outerRadius, mid - outerHalf);
    const o2 = pointAt(c, outerRadius, mid + outerHalf);
    const i1 = pointAt(c, innerRadius, mid + innerHalf);
    const i2 = pointAt(c, innerRadius, mid - innerHalf);
    const outerLarge = outerHalf * 2 > Math.PI ? 1 : 0;
    const innerLarge = innerHalf * 2 > Math.PI ? 1 : 0;
    const r2 = num(outerRadius);
    const r1 = num(innerRadius);
    return (
      `M ${num(o1.x)} ${num(o1.y)} A ${r2} ${r2} 0 ${outerLarge} 1 ${num(o2.x)} ${num(o2.y)} ` +
      `L ${num(i1.x)} ${num(i1.y)} A ${r1} ${r1} 0 ${innerLarge} 0 ${num(i2.x)} ${num(i2.y)} Z`
    );
  }

  function slicePath(c, innerRadius, outerRadius, index, count) {
    return wedgePath(c, innerRadius, outerRadius, midAngle(index, count), span(count));
  }

  /** Clockwise arc from noon covering `fraction` of a circle (the progress arc). */
  function arcPath(c, r, fraction) {
    if (fraction <= 0.001) return "";
    if (fraction >= 0.999) {
      const x = num(c.x);
      const rr = num(r);
      return `M ${x} ${num(c.y - r)} A ${rr} ${rr} 0 0 1 ${x} ${num(c.y + r)} A ${rr} ${rr} 0 0 1 ${x} ${num(c.y - r)}`;
    }
    const sweep = fraction * TAU;
    const a = pointAt(c, r, 0);
    const b = pointAt(c, r, sweep);
    const rr = num(r);
    return `M ${num(a.x)} ${num(a.y)} A ${rr} ${rr} 0 ${sweep > Math.PI ? 1 : 0} 1 ${num(b.x)} ${num(b.y)}`;
  }

  /** Inner radius for an outer radius, with Filee's hole ratio clamp. */
  function innerRadius(outerRadius, holeRatio) {
    return outerRadius * clamp(holeRatio, 0.2, 0.75);
  }

  /** Side of the square the ring needs, shadow and pop-out included. */
  function ringSide(outerRadius) {
    return 2 * (outerRadius + RING.popOut + RING.margin);
  }

  // ───────────── motion ─────────────

  /**
   * One step of a damped spring (semi-implicit Euler). Mutates `s` ({ value,
   * velocity }) and returns true while it still moves.
   */
  function stepSpring(s, target, dt, stiffness, damping) {
    // Never integrate more than 1/120 s at once. Filee learned this the hard
    // way: one fat step on a late frame and the slices "flew apart on a busy
    // machine". I'd rather not relearn it.
    const steps = Math.max(1, Math.ceil(dt * 120 - 1e-9));
    const h = dt / steps;
    for (let i = 0; i < steps; i++) {
      const force = stiffness * (target - s.value) - damping * s.velocity;
      s.velocity += force * h;
      s.value += s.velocity * h;
    }
    if (Math.abs(target - s.value) < 0.001 && Math.abs(s.velocity) < 0.01) {
      s.value = target;
      s.velocity = 0;
      return false;
    }
    return true;
  }

  /**
   * The hover spring. Becoming active is underdamped (c = 20) for the little
   * boing; letting go is critically damped (c = 36) so the accent never dips
   * below zero and flashes back.
   */
  function stepHover(s, active, dt) {
    return stepSpring(s, active ? 1 : 0, dt, 320, active ? 20 : 36);
  }

  // c1 = 1.55 overshoots a touch more than the textbook 1.70158 feels like it
  // should, but it's Filee's number and the ring is Filee's ring.
  function easeOutBack(t) {
    const c1 = 1.55;
    const c3 = c1 + 1;
    const u = t - 1;
    return 1 + c3 * u * u * u + c1 * u * u;
  }

  function easeInCubic(t) {
    return t * t * t;
  }

  /** Open value of slice `i`, `clock` seconds into the open animation. */
  function openAmount(i, clock) {
    return easeOutBack(clamp((clock - i * RING.openStagger) / RING.openSeconds, 0, 1));
  }

  /** True once every one of `count` slices finished opening. */
  function openFinished(count, clock) {
    return clock >= Math.max(0, count - 1) * RING.openStagger + RING.openSeconds;
  }

  /** Open value of slice `i`, `clock` seconds into the close animation. */
  function closeAmount(i, clock) {
    const t = clamp((clock - i * RING.closeStagger) / RING.closeSeconds, 0, 1);
    return 1 - t * t * t;
  }

  function closeFinished(count, clock) {
    return clock >= Math.max(0, count - 1) * RING.closeStagger + RING.closeSeconds;
  }

  /**
   * Transform of slice `i` while it is on screen, as plain numbers: rotation
   * (radians), scale and outward offset. `open` is the slice's open value and
   * `hover` its hover spring.
   */
  function sliceTransform(i, count, open, hover) {
    const h = Math.max(0, hover);
    const offset = pointAt({ x: 0, y: 0 }, h * RING.popOut, midAngle(i, count));
    return {
      rotate: (1 - Math.min(open, 1)) * -0.35,
      scale: (0.55 + 0.45 * open) * (1 + h * 0.025),
      dx: offset.x,
      dy: offset.y,
      opacity: clamp(open, 0, 1),
    };
  }

  // ───────────── text ─────────────

  /** Label box width for a slice, from its arc length at mid-radius. */
  function labelWidth(sliceSpan, midRadius) {
    return clamp(sliceSpan * midRadius - 10, 30, 120);
  }

  /** The caption shows only when it adds something the label doesn't say. */
  function showCaption(label, caption) {
    return !!caption && caption.toLowerCase() !== String(label || "").toLowerCase();
  }

  /**
   * Greedy word wrap into at most `maxLines` lines of `maxWidth`, measured by
   * `measure(text) -> px`. The last line ends in an ellipsis when text is left
   * over, and a word wider than a line is cut by character.
   */
  function wrapText(text, maxWidth, maxLines, measure) {
    const words = String(text || "")
      .split(/\s+/)
      .filter(Boolean);
    const lines = [];
    let line = "";
    let i = 0;
    while (i < words.length && lines.length < maxLines) {
      const candidate = line ? line + " " + words[i] : words[i];
      if (measure(candidate) <= maxWidth) {
        line = candidate;
        i++;
      } else if (line) {
        lines.push(line);
        line = "";
      } else {
        // One word that is too long by itself: it takes the line, ellipsis and all.
        lines.push(ellipsize(words[i], maxWidth, measure));
        i++;
      }
    }
    if (line && lines.length < maxLines) lines.push(line);
    if (i < words.length && lines.length > 0) {
      const last = lines.length - 1;
      lines[last] = ellipsize(lines[last] + "…", maxWidth, measure, true);
    }
    return lines;
  }

  function ellipsize(text, maxWidth, measure, forced) {
    if (!forced && measure(text) <= maxWidth) return text;
    let body = forced ? text.replace(/…$/, "") : text;
    while (body.length > 1 && measure(body + "…") > maxWidth) body = body.slice(0, -1);
    return body.trimEnd() + "…";
  }

  /** Centre subtitle for the slice under the pointer (Filee's ShowHint). */
  function hintFor(slice) {
    if (!slice) return "Esc to close";
    if (!slice.enabled) return slice.disabledReason || "Not available";
    return "Click → " + slice.label;
  }

  // ───────────── progress ─────────────

  /**
   * The progress ring's state machine, ported from DonutMenu.Progress.cs:
   * slices collapse, the ring pops in, the arc chases the real progress, the
   * outcome badge pops once the ring has been up for a moment, then it holds
   * and fades. `reduced` snaps every animation but keeps the holds, so a ✓
   * is still on screen long enough to be seen.
   *
   * One deliberate change: Filee hands a failure to its toast window. We have
   * no toast yet, so with `keepFailure` a failed ring stays up (phase "failed")
   * until the user dismisses it and can read the message.
   */
  class ProgressRing {
    constructor(sliceCount, opts) {
      const o = opts || {};
      this.count = sliceCount;
      this.reduced = !!o.reduced;
      this.keepFailure = o.keepFailure !== false;
      this.collapseEnd = RING.collapseSeconds + RING.collapseStagger * Math.max(0, sliceCount - 1);
      this.collapseClock = this.reduced ? this.collapseEnd : 0;
      this.ringClock = this.reduced ? 0 : -1;
      this.pop = { value: this.reduced ? 1 : 0, velocity: 0 };
      this.badge = { value: 0, velocity: 0 };
      this.shown = 0;
      this.target = 0;
      this.pending = null;
      this.outcome = null;
      this.holdClock = -1;
      this.fadeClock = -1;
      this.phase = "running";
    }

    setProgress(value) {
      if (this.pending) return;
      this.target = clamp(Number(value) || 0, 0, 1);
    }

    complete(outcome) {
      if (this.pending) return;
      if (outcome === "succeeded") this.target = 1;
      this.pending = outcome;
    }

    /** How far slice `i` has collapsed into the centre, 0..1. */
    collapse(i) {
      if (this.reduced) return 1;
      return easeInCubic(clamp((this.collapseClock - i * RING.collapseStagger) / RING.collapseSeconds, 0, 1));
    }

    /** 0..1 for the whole collapse, which also shrinks the hole to the plate. */
    collapseAll() {
      return this.collapseEnd <= 0 ? 1 : clamp(this.collapseClock / this.collapseEnd, 0, 1);
    }

    /** The arc's fraction: a failure still shows a sliver of red. */
    arcFraction() {
      return this.outcome === "failed" ? Math.max(this.shown, 0.02) : this.shown;
    }

    /** Ring scale and opacity after pop-in and fade. */
    ringLook() {
      const fade = this.fadeClock >= 0 ? clamp(this.fadeClock / RING.fadeSeconds, 0, 1) : 0;
      return { scale: Math.max(0, this.pop.value) * (1 - 0.15 * fade), opacity: 1 - fade };
    }

    /** Advances by `dt` seconds. Returns true while another frame is needed. */
    advance(dt) {
      if (this.phase === "done" || this.phase === "failed") return false;
      const animate = !this.reduced;
      let active = false;

      if (this.collapseClock < this.collapseEnd) {
        this.collapseClock = animate ? this.collapseClock + dt : this.collapseEnd;
        active = true;
      } else if (this.ringClock < 0) {
        this.ringClock = 0;
        // A small kick so the ring pops instead of growing politely.
        this.pop.velocity = animate ? 2 : 0;
        if (!animate) this.pop.value = 1;
      }
      if (this.ringClock >= 0) {
        this.ringClock += dt;
        if (animate) active = stepSpring(this.pop, 1, dt, 300, 16) || active;
      }

      const gap = this.target - this.shown;
      this.shown = animate ? this.shown + gap * Math.min(1, dt * 9) : this.target;
      if (Math.abs(this.target - this.shown) < 0.002) this.shown = this.target;
      else active = true;

      if (
        this.pending &&
        !this.outcome &&
        this.ringClock >= RING.minRingSeconds &&
        (this.pending !== "succeeded" || this.shown >= 0.999)
      ) {
        this.outcome = this.pending;
        this.holdClock = 0;
        if (animate) this.badge.velocity = 9;
        else this.badge.value = 1;
      }
      if (this.pending) active = true;

      if (this.outcome) {
        if (animate) stepSpring(this.badge, 1, dt, 420, 14);
        if (this.fadeClock < 0) {
          this.holdClock += dt;
          const hold = RING.holdSeconds[this.outcome] ?? RING.holdSeconds.succeeded;
          if (this.holdClock >= hold) {
            if (this.outcome === "failed" && this.keepFailure) {
              this.badge.value = 1;
              this.badge.velocity = 0;
              this.phase = "failed";
              return false;
            }
            this.fadeClock = 0;
          }
        } else {
          this.fadeClock = animate ? this.fadeClock + dt : RING.fadeSeconds;
          if (this.fadeClock >= RING.fadeSeconds) {
            this.phase = "done";
            return false;
          }
        }
        active = true;
      }
      return active;
    }
  }

  return {
    TAU,
    RING,
    clamp,
    span,
    midAngle,
    pointAt,
    angleOf,
    indexAt,
    hitTest,
    num,
    ringPath,
    wedgePath,
    slicePath,
    arcPath,
    innerRadius,
    ringSide,
    stepSpring,
    stepHover,
    easeOutBack,
    easeInCubic,
    openAmount,
    openFinished,
    closeAmount,
    closeFinished,
    sliceTransform,
    labelWidth,
    showCaption,
    wrapText,
    hintFor,
    ProgressRing,
  };
});
