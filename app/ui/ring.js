// The ring window: Filee's donut menu redrawn as one SVG. ring.math.js does
// the maths; this file owns the DOM, the frame loop and the conversation with
// the backend.
//
// Backend contract (everything goes through window.__TAURI__, no bundler):
//   events the ring listens for
//     "ring-open"     { files: string[], title: string,
//                       slices: [{ presetId, target, label, caption?, enabled,
//                                  disabledReason? }] }
//                     One slice per preset of the toolbar profile that fits the
//                     files, in the profile's order. presetId names the preset,
//                     target is its format id ("*" = same as the source) and
//                     label its display name. Rebuilds the donut and plays the
//                     open animation. Show and
//                     focus the window after emitting this; the ring is centred
//                     by the window itself, never at the cursor (Wayland).
//     "ring-progress" { jobId, progress: 0..1 }
//                     for the conversion the ring shows. The first one
//                     (progress 0) arrives as soon as the job is queued, so
//                     the ring knows which jobId to cancel.
//     "ring-done"     { outcome: "succeeded" | "failed" | "cancelled", message? }
//   commands the ring invokes
//     convert { files, presetId }
//                             runs the clicked slice's preset: its target,
//                             options and output rule, with the default output
//                             from the settings. A rejected promise counts as
//                             a failed job, and its error text is shown under
//                             the ring
//     cancel_job { jobId }    cancels the job the ring shows (Escape or a
//                             click on the centre while it runs); the backend
//                             answers with ring-done "cancelled"
//     ring_close              hides the window
//     ping                    once at start-up, as a health check
"use strict";

(function () {
  const M = window.RingMath;
  const { RING } = M;
  const SVG_NS = "http://www.w3.org/2000/svg";
  const FONT_FAMILY = '"Noto Sans", "Noto Sans KR", "Noto Sans SC", system-ui, sans-serif';

  // The SVG's viewBox is 360 square, which is also the window size in
  // tauri.conf.json. The donut needs 342 of it.
  const C = { x: 180, y: 180 };
  const R_OUT = RING.outerRadius;
  const R_IN = M.innerRadius(R_OUT, RING.holeRatio);
  const MID_R = (R_IN + R_OUT) / 2;
  const HOLE_R = R_IN - 6;
  const PLATE_R = RING.ringRadius + RING.ringStroke + 2;
  const LABEL_LINE = 16.5;
  const CAPTION_LINE = 13.5;
  const TITLE_LINE = 18;
  const SUBTITLE_LINE = 15;

  const $ = (id) => document.getElementById(id);
  const svg = $("ring");
  const slicesLayer = $("slices");
  const shadow = $("ring-shadow");
  const hole = $("hole");
  const hubTitle = $("hub-title");
  const hubSubtitle = $("hub-subtitle");
  const progressLayer = $("progress");
  const arc = $("arc");
  const percent = $("percent");
  const badge = $("badge");
  const badgeDisc = $("badge-disc");
  const badgeCheck = $("badge-check");
  const badgeBang = $("badge-bang");
  const message = $("message");

  // ───────────── state ─────────────

  let model = { files: [], title: "", slices: [] };
  let mode = "hidden"; // "menu" | "closing" | "progress" | "hidden"
  let clock = 0; // seconds into the open or close animation
  let reduced = false;
  let hover = [];
  let hoverIndex = -1;
  let pressed = null;
  let progress = null;
  let chosen = -1;
  let job = 0;
  // The backend's id for the job on the ring, from ring-progress; 0 until the
  // first one arrives. cancelWanted remembers an Escape that beat it there.
  let backendJob = 0;
  let cancelWanted = false;
  let cancelSent = false;
  let failureText = "";
  let sliceEls = [];
  // Focus-follows-mouse compositors (hello, Hyprland) hand focus to whatever
  // sits under the cursor, so a ring that pops up away from the mouse loses
  // focus on its first frame. We only treat blur as "clicked away" once the
  // pointer has actually visited the ring, and even then give it a moment.
  let pointerVisited = false;
  let blurTimer = 0;
  const BLUR_GRACE_MS = 400;
  // Ctrl+drag: the ring opened mid-drag. Until the drop, the pointer belongs to
  // the file manager, so hover and drop come in as backend events instead of
  // pointer events. Like watching a mime through a window, but with slices.
  let dragMode = false;

  // ───────────── static layout ─────────────

  svg.style.fontFamily = FONT_FAMILY;
  attrs(shadow, { cx: C.x, cy: C.y + 4, r: R_OUT + 14 });
  // Gradient stops in fractions of the shadow radius, as in DrawSoftShadow.
  $("ring-shadow-inner").setAttribute("offset", M.num((R_OUT - 18) / (R_OUT + 14)));
  $("ring-shadow-peak").setAttribute("offset", M.num((R_OUT - 2) / (R_OUT + 14)));
  attrs(hole, { cx: C.x, cy: C.y, r: HOLE_R });
  attrs($("plate-shadow"), { cx: C.x, cy: C.y + 3, r: PLATE_R + 10 });
  attrs($("plate"), { cx: C.x, cy: C.y, r: PLATE_R });
  attrs($("track"), { cx: C.x, cy: C.y, r: RING.ringRadius, "stroke-width": RING.ringStroke });
  attrs(arc, { "stroke-width": RING.ringStroke });
  attrs(percent, { x: C.x, y: C.y });
  attrs(badgeDisc, { cx: C.x, cy: C.y });
  attrs(badgeCheck, {
    points: `${C.x - 9},${C.y + 0.5} ${C.x - 2.5},${C.y + 7} ${C.x + 10},${C.y - 6.5}`,
  });
  attrs(badgeBang, { x: C.x, y: C.y + 1 });
  svg.classList.add("off");

  // ───────────── text measuring ─────────────

  const measureCtx = document.createElement("canvas").getContext("2d");
  function measurer(weight, size) {
    const font = `${weight} ${size}px ${FONT_FAMILY}`;
    return (text) => {
      measureCtx.font = font;
      return measureCtx.measureText(text).width;
    };
  }
  const measureLabel = measurer(600, 13);
  const measureCaption = measurer(400, 10.5);
  const measureTitle = measurer(600, 14);
  const measureSubtitle = measurer(400, 11.5);

  // ───────────── building ─────────────

  function normalise(payload) {
    const p = payload || {};
    const files = Array.isArray(p.files) ? p.files.map(String) : [];
    const slices = (Array.isArray(p.slices) ? p.slices : []).map((s) => ({
      // An old backend sent no presetId; its target id is the best guess, and
      // the backend finds a preset by display name too, so "png" still lands.
      presetId: String(s.presetId || s.target || ""),
      target: String(s.target || ""),
      label: String(s.label || String(s.target || "").toUpperCase()),
      caption: s.caption ? String(s.caption) : "",
      enabled: s.enabled !== false,
      disabledReason: s.disabledReason ? String(s.disabledReason) : "",
    }));
    const title =
      p.title ? String(p.title) : files.length === 1 ? "1 file" : `${files.length} files`;
    return { files, title, slices };
  }

  function build() {
    const n = model.slices.length;
    sliceEls = [];
    if (n === 0) {
      // Nothing to offer still gets a donut, just an empty one. A window that
      // pops up blank looks broken; one that says "nothing here" looks honest.
      const g = svgEl("g", { class: "slice disabled" });
      g.append(
        svgEl("path", {
          class: "wedge",
          d: M.ringPath(C, R_IN, R_OUT),
          "fill-rule": "evenodd",
          "fill-opacity": RING.sliceOpacity,
        }),
      );
      sliceEls.push({ g, accent: null });
      slicesLayer.replaceChildren(g);
      return;
    }

    const width = M.labelWidth(M.span(n), MID_R);
    const roomy = R_OUT - R_IN - 16;
    slicesLayer.replaceChildren(
      ...model.slices.map((slice, i) => {
        const d = M.slicePath(C, R_IN, R_OUT, i, n);
        const g = svgEl("g", {
          class: slice.enabled ? "slice" : "slice disabled",
          id: `slice-${i}`,
          role: "menuitem",
          "aria-label": slice.caption ? `${slice.label}, ${slice.caption}` : slice.label,
          "aria-disabled": String(!slice.enabled),
        });
        const wedge = svgEl("path", {
          class: "wedge",
          d,
          "fill-rule": "evenodd",
          "fill-opacity": RING.sliceOpacity * (slice.enabled ? 1 : 0.55),
        });
        const accent = svgEl("path", { class: "wedge-accent", d, "fill-rule": "evenodd", opacity: 0 });
        g.append(wedge, accent, ...labelText(slice, i, n, width, roomy));
        sliceEls.push({ g, accent });
        return g;
      }),
    );
  }

  function labelText(slice, i, n, width, roomy) {
    const anchor = M.pointAt(C, MID_R, M.midAngle(i, n));
    const lines = M.wrapText(slice.label, width, 2, measureLabel);
    const labelHeight = lines.length * LABEL_LINE;
    const caption =
      M.showCaption(slice.label, slice.caption) && labelHeight + 14 < roomy
        ? M.wrapText(slice.caption, width, 1, measureCaption)
        : [];
    const total = labelHeight + caption.length * CAPTION_LINE;
    const top = anchor.y - total / 2;
    const out = [];
    if (lines.length) {
      out.push(textBlock("slice-label", lines, anchor.x, top, LABEL_LINE));
    }
    if (caption.length) {
      out.push(textBlock("slice-caption", caption, anchor.x, top + labelHeight, CAPTION_LINE));
    }
    return out;
  }

  // Centring with text-anchor="middle" on the anchor point itself sidesteps
  // the bug Filee fixed in DrawCentered, where short labels drifted right
  // because the measured text, not its box, got centred.
  function textBlock(cls, lines, x, top, lineHeight) {
    const text = svgEl("text", { class: cls, "text-anchor": "middle" });
    fillLines(text, lines, x, top, lineHeight);
    return text;
  }

  function fillLines(text, lines, x, top, lineHeight) {
    text.replaceChildren(
      ...lines.map((line, k) => {
        const span = svgEl("tspan", {
          x: M.num(x),
          y: M.num(top + lineHeight * (k + 0.5)),
          "dominant-baseline": "central",
        });
        span.textContent = line;
        return span;
      }),
    );
  }

  function setHub(title, subtitle) {
    const maxWidth = HOLE_R * 1.6;
    const t = M.wrapText(title, maxWidth, 2, measureTitle);
    const s = subtitle ? M.wrapText(subtitle, maxWidth, 2, measureSubtitle) : [];
    const total = t.length * TITLE_LINE + (s.length ? s.length * SUBTITLE_LINE + 2 : 0);
    const top = C.y - total / 2;
    fillLines(hubTitle, t, C.x, top, TITLE_LINE);
    fillLines(hubSubtitle, s, C.x, top + t.length * TITLE_LINE + 2, SUBTITLE_LINE);
  }

  // ───────────── opening and closing ─────────────

  function openRing(payload) {
    model = normalise(payload);
    // Re-read on every open, like Filee's Motion.Refresh: a system that said
    // "no animations" once shouldn't get its way forever.
    reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    mode = "menu";
    clock = 0;
    hoverIndex = -1;
    pressed = null;
    progress = null;
    chosen = -1;
    job++;
    hover = model.slices.map(() => ({ value: 0, velocity: 0 }));
    pointerVisited = false;
    clearTimeout(blurTimer);
    message.hidden = true;
    svg.removeAttribute("aria-busy");
    svg.removeAttribute("aria-activedescendant");
    svg.classList.remove("pointing", "cancelling");
    build();
    setHub(model.title, M.hintFor(null));
    svg.classList.remove("off");
    render();
    startAnimation();
  }

  function requestClose() {
    if (mode === "menu" && !reduced) {
      mode = "closing";
      clock = 0;
      startAnimation();
    } else if (mode === "menu" || mode === "progress") {
      // Closing mid-conversion (by clicking away) only hides the ring. The
      // job carries on in the backend; we just stop staring at it.
      hide();
    }
  }

  function hide() {
    mode = "hidden";
    dragMode = false;
    job++;
    message.hidden = true;
    svg.classList.remove("cancelling", "pointing");
    svg.classList.add("off");
    backend.close().catch((err) => console.warn("ring_close failed:", err));
  }

  // ───────────── Ctrl+drag ─────────────

  function dragHint(slice) {
    if (!slice) return "Drop on a format";
    return slice.enabled ? `Drop → ${slice.label}` : M.hintFor(slice);
  }

  function dragAt(p) {
    if (mode !== "menu" || model.slices.length === 0 || !p) return { kind: "outside" };
    return hitAt({ clientX: Number(p.x), clientY: Number(p.y) });
  }

  function armRing() {
    openRing({ files: [], title: "Drop here", slices: [] });
    dragMode = true;
    setHub("Drop here", "Then pick a format");
  }

  function dragEnter(payload) {
    if (!dragMode) return;
    openRing(payload);
    setHub(model.title, dragHint(null));
  }

  function dragOver(p) {
    if (!dragMode || mode !== "menu") return;
    const hit = dragAt(p);
    setHover(hit.kind === "segment" ? hit.index : -1);
    setHub(model.title, dragHint(model.slices[hoverIndex]));
  }

  function dragLeave() {
    if (!dragMode || mode !== "menu") return;
    setHover(-1);
    setHub(model.title, dragHint(null));
  }

  function dragDrop(p) {
    if (!dragMode) return;
    dragMode = false;
    const hit = dragAt(p);
    const slice = hit.kind === "segment" ? model.slices[hit.index] : null;
    if (slice && slice.enabled) invoke(hit.index);
    else requestClose();
  }

  function dragDismiss() {
    if (!dragMode) return;
    dragMode = false;
    requestClose();
  }

  // ───────────── converting ─────────────

  function invoke(index) {
    const slice = model.slices[index];
    if (mode !== "menu" || !slice || !slice.enabled) return;
    chosen = index;
    mode = "progress";
    progress = new M.ProgressRing(model.slices.length, { reduced });
    failureText = "";
    backendJob = 0;
    cancelWanted = false;
    cancelSent = false;
    svg.setAttribute("aria-busy", "true");
    svg.classList.remove("pointing");
    startAnimation();
    const mine = job;
    backend.convert(model.files, slice.presetId).catch((err) => {
      if (mine === job) finish("failed", errorText(err));
    });
  }

  // Whether the job on the ring is still going, i.e. cancelling means something.
  function running() {
    return mode === "progress" && !!progress && !progress.pending;
  }

  // Escape or a click on the centre while the job runs. The ring greys out
  // at once, then fades like any other cancelled job when ring-done says so.
  // Asking twice is fine; the second ask just closes the ring.
  function requestCancel() {
    if (!running() || cancelWanted) {
      requestClose();
      return;
    }
    cancelWanted = true;
    svg.classList.add("cancelling");
    svg.classList.remove("pointing");
    sendCancel();
  }

  function sendCancel() {
    if (!cancelWanted || cancelSent || !backendJob) return;
    cancelSent = true;
    const mine = job;
    backend.cancel(backendJob).catch((err) => {
      console.warn("cancel_job failed:", err);
      // The job runs on, so stop pretending otherwise.
      if (mine !== job) return;
      cancelWanted = false;
      cancelSent = false;
      svg.classList.remove("cancelling");
    });
  }

  function finish(outcome, text) {
    // First word wins: a late ring-done must not rewrite an error already shown.
    if (mode !== "progress" || !progress || progress.pending) return;
    const known = outcome === "succeeded" || outcome === "cancelled";
    failureText = text || "";
    progress.complete(known ? outcome : "failed");
    svg.classList.remove("pointing");
    startAnimation();
  }

  function errorText(err) {
    if (typeof err === "string") return err;
    if (err && typeof err.message === "string") return err.message;
    return "The conversion failed.";
  }

  // ───────────── frame loop ─────────────

  let rafId = 0;
  let lastTs = null;
  let lastFrameAt = 0;
  let stalls = 0;
  let watchdog = 0;

  function startAnimation() {
    // A loop that is still getting frames needs no help. One that went quiet
    // gets replaced, and the new animation starts its stall count from zero,
    // the way Filee's StartAnimation does it.
    if (rafId && performance.now() - lastFrameAt < M.WATCHDOG.stalledMs) return;
    if (rafId) cancelAnimationFrame(rafId);
    stalls = 0;
    lastFrameAt = performance.now();
    rafId = requestAnimationFrame(frame);
    armWatchdog();
  }

  function frame(ts) {
    // The first frame gets a polite 1/60 s; later ones are capped at 50 ms
    // so a hiccup can't fling the springs.
    step(ts, 0.05);
  }

  function step(now, maxDt) {
    rafId = 0;
    const dt = lastTs === null ? 1 / 60 : M.clamp((now - lastTs) / 1000, 0, maxDt);
    lastTs = now;
    lastFrameAt = performance.now();
    const active = advance(dt);
    render();
    if (active && mode !== "hidden") {
      rafId = requestAnimationFrame(frame);
    } else {
      lastTs = null;
      clearTimeout(watchdog);
    }
  }

  // Frames can go missing: webviews throttle requestAnimationFrame in hidden
  // or transparent windows, and a request made just before a hide can simply
  // never be answered. Filee's answer, and mine: ask again a couple of times,
  // and if the loop is still asleep, jump every animation to its end so an
  // opening ring always actually appears. Stalls count per animation, not per
  // frame, or frames trickling in once per retry would open the donut in slow
  // motion. The timers (minimum ring time, the check-mark hold) keep ticking
  // from here at real speed, or a stalled ring would never finish.
  function armWatchdog() {
    clearTimeout(watchdog);
    watchdog = setTimeout(checkLoop, M.WATCHDOG.checkMs);
  }

  function checkLoop() {
    if (!rafId) return;
    const now = performance.now();
    const check = M.watchdogCheck(now, lastFrameAt, stalls);
    stalls = check.stalls;
    if (check.action !== "watch") {
      cancelAnimationFrame(rafId);
      rafId = 0;
      if (check.action === "finish") finishAnimations();
      if (mode !== "hidden") step(now, 0.3);
      else lastTs = null;
    }
    if (rafId) armWatchdog();
  }

  function finishAnimations() {
    const state = { mode, clock, hover, hoverIndex, progress };
    const done = M.finishAll(state);
    clock = state.clock;
    if (done) hide();
  }

  function advance(dt) {
    const n = model.slices.length;
    let active = false;
    if (mode === "menu") {
      clock += dt;
      if (!reduced && !M.openFinished(n, clock)) active = true;
      hover.forEach((s, i) => {
        const on = i === hoverIndex;
        if (reduced) {
          s.value = on ? 1 : 0;
          s.velocity = 0;
        } else if (M.stepHover(s, on, dt)) {
          active = true;
        }
      });
    } else if (mode === "closing") {
      clock += dt;
      if (M.closeFinished(n, clock)) {
        hide();
        return false;
      }
      active = true;
    } else if (mode === "progress" && progress) {
      active = progress.advance(dt);
      if (progress.outcome === "failed" && message.hidden) {
        message.textContent = failureText || "The conversion failed.";
        message.hidden = false;
      }
      if (progress.phase === "done") {
        hide();
        return false;
      }
    }
    return active;
  }

  // ───────────── drawing ─────────────

  function render() {
    if (mode === "hidden") return;
    if (mode === "progress" && progress) {
      renderProgress();
      return;
    }
    const n = model.slices.length;
    let openSum = 0;
    sliceEls.forEach((el, i) => {
      const open = sliceOpen(i);
      openSum += M.clamp(open, 0, 1);
      if (open <= 0.001) {
        setVisible(el.g, false);
        return;
      }
      setVisible(el.g, true);
      const h = n ? hover[i].value : 0;
      const t = M.sliceTransform(i, Math.max(n, 1), open, h);
      place(el.g, t.rotate, t.scale, t.dx, t.dy, t.opacity);
      if (el.accent) {
        const enabled = model.slices[i].enabled;
        el.accent.setAttribute("opacity", M.num(enabled ? M.clamp(h, 0, 1) : 0));
        el.g.classList.toggle("hot", enabled && i === hoverIndex);
      }
    });
    shadow.setAttribute("opacity", M.num(sliceEls.length ? openSum / sliceEls.length : 1));
    hole.setAttribute("r", M.num(HOLE_R));
    setVisible(hole, true);
    setVisible(hubTitle, true);
    setVisible(hubSubtitle, true);
    setVisible(progressLayer, false);
  }

  function sliceOpen(i) {
    if (mode === "menu") return reduced ? 1 : M.openAmount(i, clock);
    if (mode === "closing") return M.closeAmount(i, clock);
    return 1;
  }

  function renderProgress() {
    const all = progress.collapseAll();
    setVisible(hubTitle, false);
    setVisible(hubSubtitle, false);

    if (all < 1) {
      // Slices get sucked into the hub with a little twist, then the hub
      // shrinks to the ring's plate.
      shadow.setAttribute("opacity", M.num(1 - all));
      sliceEls.forEach((el, i) => {
        const c = progress.collapse(i);
        if (c >= 0.999) {
          setVisible(el.g, false);
          return;
        }
        setVisible(el.g, true);
        place(el.g, c * 0.25, 1 - 0.8 * c, 0, 0, 1 - c);
        if (el.accent) el.accent.setAttribute("opacity", i === chosen ? "1" : "0");
        el.g.classList.toggle("hot", i === chosen);
      });
      hole.setAttribute("r", M.num(HOLE_R + (PLATE_R - HOLE_R) * M.easeInCubic(all)));
      setVisible(hole, true);
      setVisible(progressLayer, false);
      return;
    }

    sliceEls.forEach((el) => setVisible(el.g, false));
    shadow.setAttribute("opacity", "0");
    setVisible(hole, false);
    setVisible(progressLayer, true);

    const look = progress.ringLook();
    place(progressLayer, 0, look.scale, 0, 0, look.opacity);
    const outcome = progress.outcome;
    arc.setAttribute("d", M.arcPath(C, RING.ringRadius, progress.arcFraction()));
    arc.setAttribute("class", outcome ? `arc ${outcome}` : "arc");

    const b = outcome ? Math.max(0, progress.badge.value) : 0;
    percent.textContent = `${Math.round(progress.shown * 100)}%`;
    percent.setAttribute("opacity", M.num(1 - M.clamp(b, 0, 1)));

    const showBadge = outcome && outcome !== "cancelled" && b > 0.01;
    setVisible(badge, !!showBadge);
    if (showBadge) {
      badgeDisc.setAttribute("r", M.num(22 * b));
      badgeDisc.setAttribute("class", outcome === "failed" ? "badge-disc failed" : "badge-disc");
      const mark = `translate(${C.x} ${C.y}) scale(${M.num(b)}) translate(${-C.x} ${-C.y})`;
      badgeCheck.setAttribute("transform", mark);
      badgeBang.setAttribute("transform", mark);
      setVisible(badgeCheck, outcome === "succeeded");
      setVisible(badgeBang, outcome === "failed");
    }
  }

  // Rotate and scale about the ring's centre, then nudge by (dx, dy). SVG
  // applies the list right to left, so read it backwards.
  function place(el, rotateRad, scale, dx, dy, opacity) {
    const deg = (rotateRad * 180) / Math.PI;
    el.setAttribute(
      "transform",
      `translate(${M.num(C.x + dx)} ${M.num(C.y + dy)}) scale(${M.num(scale)}) rotate(${M.num(deg)}) translate(${-C.x} ${-C.y})`,
    );
    el.setAttribute("opacity", M.num(opacity));
  }

  // ───────────── input ─────────────

  function hitAt(e) {
    const ctm = svg.getScreenCTM();
    if (!ctm) return { kind: "outside" };
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(ctm.inverse());
    if (mode === "progress") {
      return Math.hypot(p.x - C.x, p.y - C.y) <= PLATE_R ? { kind: "ring" } : { kind: "outside" };
    }
    // Slack of one pop-out, so a slice that jumped towards the pointer is
    // still the slice under it.
    return M.hitTest(C, p, R_IN, R_OUT, model.slices.length, RING.popOut);
  }

  function setHover(index) {
    if (index === hoverIndex) return;
    hoverIndex = index;
    const slice = model.slices[index];
    svg.classList.toggle("pointing", !!slice && slice.enabled);
    if (slice) svg.setAttribute("aria-activedescendant", `slice-${index}`);
    else svg.removeAttribute("aria-activedescendant");
    setHub(model.title, M.hintFor(slice));
    startAnimation();
  }

  svg.addEventListener("pointermove", (e) => {
    if (mode === "progress") {
      // The centre is a cancel button while the job runs; say so with the cursor.
      svg.classList.toggle("pointing", running() && !cancelWanted && hitAt(e).kind === "ring");
      return;
    }
    if (mode !== "menu") return;
    const hit = hitAt(e);
    setHover(hit.kind === "segment" ? hit.index : -1);
  });

  svg.addEventListener("pointerleave", () => {
    if (mode === "menu" && !pressed) setHover(-1);
  });

  svg.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    pressed = hitAt(e);
  });

  svg.addEventListener("pointerup", (e) => {
    if (e.button !== 0 || !pressed) return;
    pressed = null;
    const hit = hitAt(e);
    if (mode === "menu") {
      if (hit.kind === "segment") invoke(hit.index);
      // A click in the hole, or in the empty corners of the window, closes.
      // Filee lets corner clicks fall through to whatever is behind; a
      // WebKitGTK window can't, so the next best thing is to get out of the way.
      else if (hit.kind !== "empty") requestClose();
    } else if (mode === "progress" && progress && progress.phase === "failed") {
      requestClose();
    } else if (running() && hit.kind === "ring") {
      requestCancel();
    }
  });

  // No browser context menu on a ring. There is nothing to "Inspect" here.
  svg.addEventListener("contextmenu", (e) => e.preventDefault());

  document.addEventListener("keydown", (e) => {
    if (mode === "hidden") return;
    if (e.key === "Escape") {
      e.preventDefault();
      // Mid-conversion, Escape means "stop that"; anywhere else, "go away".
      if (running()) requestCancel();
      else requestClose();
      return;
    }
    if (mode === "progress" && progress && progress.phase === "failed" && e.key === "Enter") {
      requestClose();
      return;
    }
    const n = model.slices.length;
    if (mode !== "menu" || n === 0) return;
    if (e.key === "ArrowRight" || e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) {
      e.preventDefault();
      setHover(hoverIndex < 0 ? 0 : (hoverIndex + 1) % n);
    } else if (e.key === "ArrowLeft" || e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) {
      e.preventDefault();
      setHover(hoverIndex < 0 ? n - 1 : (hoverIndex - 1 + n) % n);
    } else if ((e.key === "Enter" || e.key === " ") && hoverIndex >= 0) {
      e.preventDefault();
      invoke(hoverIndex);
    }
  });

  // Click mode closes when the window loses focus for real: the pointer has
  // been on the ring, and focus stays gone past a short grace period (so a
  // twitchy compositor or a mouse crossing the edge doesn't kill it).
  document.addEventListener("pointerenter", () => {
    pointerVisited = true;
  });
  document.addEventListener("pointermove", () => {
    pointerVisited = true;
  });
  window.addEventListener("focus", () => clearTimeout(blurTimer));
  window.addEventListener("blur", () => {
    if (!pointerVisited || mode === "hidden") return;
    clearTimeout(blurTimer);
    blurTimer = setTimeout(() => {
      if (!document.hasFocus() && mode !== "hidden") requestClose();
    }, BLUR_GRACE_MS);
  });

  // ───────────── backends ─────────────

  function tauriBackend(tauri) {
    const on = (name, handler) =>
      tauri.event.listen(name, (event) => handler(event.payload)).catch((err) => {
        console.warn(`could not listen for ${name}:`, err);
      });
    on("ring-open", (payload) => {
      dragMode = false;
      openRing(payload);
    });
    on("ring-arm", armRing);
    on("ring-drag-enter", dragEnter);
    on("ring-drag-over", dragOver);
    on("ring-drag-leave", dragLeave);
    on("ring-drag-drop", dragDrop);
    on("ring-drag-dismiss", dragDismiss);
    on("ring-progress", (p) => {
      if (mode !== "progress" || !progress) return;
      const id = Number(p && p.jobId);
      if (id > 0 && id !== backendJob) {
        backendJob = id;
        sendCancel();
      }
      progress.setProgress(p && p.progress);
      startAnimation();
    });
    on("ring-done", (p) => finish(p && p.outcome, p && p.message));
    tauri.core.invoke("ping").catch((err) => console.warn("backend did not answer ping:", err));
    return {
      convert: (files, presetId) => tauri.core.invoke("convert", { files, presetId }),
      cancel: (jobId) => tauri.core.invoke("cancel_job", { jobId }),
      close: () => tauri.core.invoke("ring_close"),
    };
  }

  // Opened in a plain browser: show a sample ring so the look can be tuned
  // without building the app. Conversions are pretend, obviously; the last
  // enabled slice pretends to fail so the error state gets some love too.
  function demoBackend() {
    const sample = {
      files: ["/home/me/Pictures/holiday.heic", "/home/me/Pictures/beach.heic"],
      title: "2 × HEIC",
      slices: [
        { presetId: "to-jpg", target: "jpg", label: "JPG", caption: "JPEG image", enabled: true },
        { presetId: "to-png", target: "png", label: "PNG", caption: "PNG image", enabled: true },
        { presetId: "to-webp", target: "webp", label: "WebP", caption: "WebP image", enabled: true },
        { presetId: "to-avif", target: "avif", label: "AVIF", caption: "AV1 image", enabled: true },
        {
          presetId: "to-pdf",
          target: "pdf",
          label: "PDF",
          caption: "PDF document",
          enabled: false,
          disabledReason: "Needs LibreOffice: pacman -S libreoffice-fresh",
        },
        { presetId: "to-gif", target: "gif", label: "GIF", caption: "GIF image", enabled: true },
      ],
    };
    setTimeout(() => openRing(sample), 50);
    let demoJob = 0;
    let stopDemo = null;
    return {
      convert: (files, presetId) =>
        new Promise((resolve, reject) => {
          demoJob++;
          backendJob = demoJob;
          sendCancel();
          if (presetId === "to-gif") {
            setTimeout(() => reject("magick exited with code 1: no images defined"), 900);
            return;
          }
          let p = 0;
          const tick = setInterval(() => {
            p = Math.min(1, p + 0.03);
            if (mode === "progress" && progress) {
              progress.setProgress(p);
              startAnimation();
            }
            if (p >= 1) {
              clearInterval(tick);
              stopDemo = null;
              finish("succeeded");
              resolve();
            }
          }, 90);
          stopDemo = () => {
            clearInterval(tick);
            stopDemo = null;
            finish("cancelled");
            resolve();
          };
        }),
      cancel: () => {
        if (stopDemo) setTimeout(stopDemo, 150);
        return Promise.resolve();
      },
      close: () => {
        setTimeout(() => openRing(sample), 700);
        return Promise.resolve();
      },
    };
  }

  // ───────────── helpers ─────────────

  function svgEl(tag, attributes) {
    return attrs(document.createElementNS(SVG_NS, tag), attributes);
  }

  function attrs(el, attributes) {
    for (const [k, v] of Object.entries(attributes || {})) el.setAttribute(k, String(v));
    return el;
  }

  // display, not visibility: a child with visibility="visible" shows through
  // a hidden parent, which is a fun way to get a floating ✓ with no ring.
  function setVisible(el, on) {
    if (on) el.removeAttribute("display");
    else el.setAttribute("display", "none");
  }

  // Last, so every function and binding above exists before the first event
  // can arrive.
  const backend = window.__TAURI__ ? tauriBackend(window.__TAURI__) : demoBackend();
})();
