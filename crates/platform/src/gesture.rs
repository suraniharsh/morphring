//! The gesture state machine that decides when the ring opens.
//!
//! A port of Filee's `GestureDetector` and its `TriggerGesture` settings
//! model. It turns low-level mouse and keyboard events into "open the ring"
//! events and knows nothing about hooks, windows, threads or clocks, so every
//! rule can be tested with plain function calls.
//!
//! ```text
//! Drag:      button down with the exact modifiers -> (armed) -> moved past the
//!            threshold -> DragStarted -> button up -> DragEnded
//! Hold:      button down with the exact modifiers -> (armed) -> RequestHoldTimer
//!            -> hold_elapsed(token) without moving -> HoldTriggered
//! Key chord: key down with the exact modifiers -> KeyChordTriggered at the
//!            last known cursor position
//! ```
//!
//! Every input method returns the events it produced, in order. The owner
//! runs the hold timer itself: on [`GestureEvent::RequestHoldTimer`] it waits
//! `ms` milliseconds and then calls [`Detector::hold_elapsed`] with the token.
//!
//! [`Detector`] takes `&mut self` everywhere; an owner that feeds it from a
//! hook thread wraps it in a `Mutex`.

use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::ops::{BitAnd, BitOr, BitOrAssign};
use std::sync::atomic::{AtomicU64, Ordering};

/// A set of modifier keys. Left and right variants count as the same key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(u8);

// Hand-rolled bitflags: four bits did not seem worth a dependency, and the
// values match Filee's `ModifierKeys` so its settings files read the same.
impl Modifiers {
    /// No modifier held.
    pub const NONE: Self = Self(0);
    /// Control.
    pub const CTRL: Self = Self(1);
    /// Alt (Option on macOS).
    pub const ALT: Self = Self(2);
    /// Shift.
    pub const SHIFT: Self = Self(4);
    /// The Windows key on Windows, Command on macOS, Super on Linux.
    pub const META: Self = Self(8);

    const ALL: u8 = 0b1111;

    /// The raw bits, in Filee's encoding.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Builds a set from raw bits, dropping any unknown bit.
    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self(bits & Self::ALL)
    }

    /// True when no modifier is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when every modifier in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Modifiers {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl fmt::Debug for Modifiers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("Modifiers(NONE)");
        }
        let names = [
            (Self::CTRL, "CTRL"),
            (Self::ALT, "ALT"),
            (Self::SHIFT, "SHIFT"),
            (Self::META, "META"),
        ];
        let set: Vec<&str> = names
            .iter()
            .filter(|(flag, _)| self.contains(*flag))
            .map(|(_, name)| *name)
            .collect();
        write!(f, "Modifiers({})", set.join(" | "))
    }
}

/// A mouse button a gesture can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Button {
    /// No button. Key chords use this, and so does a move with nothing held.
    None,
    /// The primary button.
    #[default]
    Left,
    /// The secondary button.
    Right,
    /// The wheel button.
    Middle,
    /// The first side button ("back").
    X1,
    /// The second side button ("forward").
    X2,
}

/// The physical action that completes a gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GestureKind {
    /// Hold the modifiers and drag with the button past the threshold.
    #[default]
    Drag,
    /// Hold the modifiers and keep the button pressed, without moving, for
    /// [`TriggerGesture::hold_ms`].
    Hold,
    /// Press the modifiers plus [`TriggerGesture::key`], no mouse involved.
    KeyChord,
}

/// Where a gesture is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Scope {
    /// Only over the system file manager or the desktop.
    #[default]
    FileManager,
    /// Everywhere except excluded applications.
    Anywhere,
}

/// A user-configurable gesture that opens the ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerGesture {
    /// Short random identifier (8 hex digits).
    pub id: String,
    /// Disabled gestures are ignored by [`Detector`].
    pub enabled: bool,
    /// What completes the gesture.
    pub kind: GestureKind,
    /// The exact modifier set that must be held.
    pub modifiers: Modifiers,
    /// The mouse button; [`Button::None`] for key chords.
    pub button: Button,
    /// Extra non-modifier key name, for example `"Space"`. Required for
    /// [`GestureKind::KeyChord`], optional otherwise. Matched ignoring ASCII
    /// case.
    pub key: Option<String>,
    /// Pixels the pointer must travel before a drag counts. 0 means the
    /// system default.
    pub drag_threshold: u32,
    /// Press duration for [`GestureKind::Hold`], in milliseconds.
    pub hold_ms: u32,
    /// Where the gesture is active.
    pub scope: Scope,
}

impl Default for TriggerGesture {
    fn default() -> Self {
        Self {
            id: new_id(),
            enabled: true,
            kind: GestureKind::Drag,
            modifiers: Modifiers::CTRL,
            button: Button::Left,
            key: None,
            drag_threshold: 0,
            hold_ms: DEFAULT_HOLD_MS,
            scope: Scope::FileManager,
        }
    }
}

/// Default hold duration, in milliseconds.
pub const DEFAULT_HOLD_MS: u32 = 450;
/// Largest drag threshold the settings accept, in pixels.
pub const MAX_DRAG_THRESHOLD: u32 = 200;
/// Shortest hold the settings accept, in milliseconds.
pub const MIN_HOLD_MS: u32 = 150;
/// Longest hold the settings accept, in milliseconds.
pub const MAX_HOLD_MS: u32 = 3000;

impl TriggerGesture {
    /// The default gestures for the OS this was built for: a modifier drag
    /// with the left button, and Ctrl+Alt+Space.
    pub fn defaults() -> Vec<Self> {
        // On macOS Ctrl+click is a right click and Option already means
        // "copy" in Finder, so the drag borrows Option instead. Everyone
        // else gets Ctrl.
        let drag_modifier = if cfg!(target_os = "macos") {
            Modifiers::ALT
        } else {
            Modifiers::CTRL
        };
        vec![
            Self {
                kind: GestureKind::Drag,
                modifiers: drag_modifier,
                button: Button::Left,
                ..Self::default()
            },
            Self {
                kind: GestureKind::KeyChord,
                modifiers: Modifiers::CTRL | Modifiers::ALT,
                button: Button::None,
                key: Some("Space".to_owned()),
                ..Self::default()
            },
        ]
    }

    /// Applies the limits Filee enforces when saving from the settings UI:
    /// threshold 0..=200, hold 150..=3000 ms, and no button on key chords.
    pub fn sanitized(mut self) -> Self {
        self.drag_threshold = self.drag_threshold.min(MAX_DRAG_THRESHOLD);
        self.hold_ms = self.hold_ms.clamp(MIN_HOLD_MS, MAX_HOLD_MS);
        if self.kind == GestureKind::KeyChord {
            self.button = Button::None;
        }
        self
    }

    fn key_matches(&self, key: &str) -> bool {
        self.key
            .as_deref()
            .is_some_and(|own| own.eq_ignore_ascii_case(key))
    }
}

/// Eight hex digits that are different every call. Filee slices a GUID; we
/// have no `rand`, so std's randomly keyed hasher eats a counter instead.
/// Good enough to tell two gestures apart, nowhere near good enough for
/// anything with "crypto" in its name.
fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    format!("{:08x}", hasher.finish() as u32)
}

/// Something the detector wants its owner to know or do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GestureEvent {
    /// A drag gesture passed its threshold (or its modifier arrived late).
    DragStarted {
        /// The gesture that fired.
        gesture: TriggerGesture,
        /// Pointer position when it fired.
        x: i32,
        /// Pointer position when it fired.
        y: i32,
    },
    /// The button of an active drag gesture was released.
    DragEnded {
        /// Pointer position at release.
        x: i32,
        /// Pointer position at release.
        y: i32,
    },
    /// A hold gesture's timer elapsed without the pointer moving.
    HoldTriggered {
        /// The gesture that fired.
        gesture: TriggerGesture,
        /// Where the button went down.
        x: i32,
        /// Where the button went down.
        y: i32,
    },
    /// A key chord was pressed.
    KeyChordTriggered {
        /// The gesture that fired.
        gesture: TriggerGesture,
        /// Last known pointer position.
        x: i32,
        /// Last known pointer position.
        y: i32,
    },
    /// A hold gesture was armed: call [`Detector::hold_elapsed`] with `token`
    /// after `ms` milliseconds.
    RequestHoldTimer {
        /// Identifies this press; stale tokens are ignored.
        token: u64,
        /// Delay before calling back, in milliseconds.
        ms: u32,
    },
}

/// Decides whether a gesture may start at a screen point (scope, excluded
/// apps, our own windows). It runs on every press, so it must be fast.
pub type ScopeFilter = Box<dyn Fn(&TriggerGesture, i32, i32) -> bool + Send>;

/// A press that matched a gesture and is waiting to become one.
#[derive(Debug, Clone, Copy)]
struct Armed {
    /// Index into `Detector::gestures`. Safe because `configure` disarms.
    gesture: usize,
    x: i32,
    y: i32,
    token: u64,
}

/// Detects configured [`TriggerGesture`]s in a stream of input events.
pub struct Detector {
    gestures: Vec<TriggerGesture>,
    system_drag_threshold: u32,
    scope_filter: ScopeFilter,
    paused: bool,
    armed: Option<Armed>,
    drag_active: bool,
    /// The scope check failed for this drag; don't ask again on every move.
    late_rejected: bool,
    last_x: i32,
    last_y: i32,
    next_token: u64,
}

impl fmt::Debug for Detector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Detector")
            .field("gestures", &self.gestures)
            .field("system_drag_threshold", &self.system_drag_threshold)
            .field("paused", &self.paused)
            .field("armed", &self.armed)
            .field("drag_active", &self.drag_active)
            .field("late_rejected", &self.late_rejected)
            .finish_non_exhaustive()
    }
}

impl Detector {
    /// Creates a detector for the enabled gestures in `gestures`.
    ///
    /// `system_drag_threshold` (pixels) applies to gestures whose own
    /// threshold is 0. `scope_filter` is asked before any gesture starts.
    pub fn new(
        gestures: impl IntoIterator<Item = TriggerGesture>,
        system_drag_threshold: u32,
        scope_filter: ScopeFilter,
    ) -> Self {
        let mut detector = Self {
            gestures: Vec::new(),
            system_drag_threshold,
            scope_filter,
            paused: false,
            armed: None,
            drag_active: false,
            late_rejected: false,
            last_x: 0,
            last_y: 0,
            next_token: 0,
        };
        detector.configure(gestures);
        detector
    }

    /// Replaces the gesture list (disabled gestures are dropped) and
    /// disarms any pending press.
    pub fn configure(&mut self, gestures: impl IntoIterator<Item = TriggerGesture>) {
        self.gestures = gestures.into_iter().filter(|g| g.enabled).collect();
        self.armed = None;
    }

    /// The gestures the detector is watching for.
    pub fn gestures(&self) -> &[TriggerGesture] {
        &self.gestures
    }

    /// Replaces the scope filter.
    pub fn set_scope_filter(&mut self, scope_filter: ScopeFilter) {
        self.scope_filter = scope_filter;
    }

    /// Changes the fallback drag threshold, in pixels.
    pub fn set_system_drag_threshold(&mut self, pixels: u32) {
        self.system_drag_threshold = pixels;
    }

    /// True while detection is paused.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Pauses or resumes detection (the tray's "Pause").
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// A mouse button went down at `(x, y)` with `modifiers` held.
    pub fn mouse_down(
        &mut self,
        button: Button,
        x: i32,
        y: i32,
        modifiers: Modifiers,
    ) -> Vec<GestureEvent> {
        self.last_x = x;
        self.last_y = y;
        self.armed = None;
        self.late_rejected = false;
        if self.paused {
            return Vec::new();
        }

        let Some(index) = self.gestures.iter().position(|g| {
            matches!(g.kind, GestureKind::Drag | GestureKind::Hold)
                && g.button == button
                && g.modifiers == modifiers
        }) else {
            return Vec::new();
        };
        let gesture = &self.gestures[index];
        if !(self.scope_filter)(gesture, x, y) {
            return Vec::new();
        }

        // Filee mixes the tick count with the position. We have no clock, and
        // a counter is a better "is this the same press?" answer anyway.
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.armed = Some(Armed {
            gesture: index,
            x,
            y,
            token,
        });
        if gesture.kind == GestureKind::Hold {
            vec![GestureEvent::RequestHoldTimer {
                token,
                ms: gesture.hold_ms,
            }]
        } else {
            Vec::new()
        }
    }

    /// The pointer moved to `(x, y)`. `held_button` is the button currently
    /// held, or [`Button::None`].
    pub fn mouse_move(
        &mut self,
        x: i32,
        y: i32,
        modifiers: Modifiers,
        held_button: Button,
    ) -> Vec<GestureEvent> {
        self.last_x = x;
        self.last_y = y;
        let fired = match self.armed {
            Some(armed) => self.detect_drag_past_threshold(armed, x, y),
            None => self.detect_late_modifier(x, y, modifiers, held_button),
        };
        match fired {
            Some(index) => {
                self.drag_active = true;
                vec![GestureEvent::DragStarted {
                    gesture: self.gestures[index].clone(),
                    x,
                    y,
                }]
            }
            None => Vec::new(),
        }
    }

    /// An armed press moved. A drag fires once it leaves the threshold
    /// square; a hold is simply cancelled, because a hold that wanders off
    /// is not a hold, it's a drag with commitment issues.
    fn detect_drag_past_threshold(&mut self, armed: Armed, x: i32, y: i32) -> Option<usize> {
        let gesture = &self.gestures[armed.gesture];
        let threshold = if gesture.drag_threshold > 0 {
            gesture.drag_threshold
        } else {
            self.system_drag_threshold
        };
        let threshold = i64::from(threshold);
        // `<=` on purpose: landing exactly on the threshold is still "no move".
        // i64 so a pointer at i32::MIN cannot overflow the subtraction.
        let dx = (i64::from(x) - i64::from(armed.x)).abs();
        let dy = (i64::from(y) - i64::from(armed.y)).abs();
        if dx <= threshold && dy <= threshold {
            return None;
        }

        self.armed = None;
        (gesture.kind == GestureKind::Drag).then_some(armed.gesture)
    }

    /// The modifier arrived after the drag was already under way (common in
    /// Explorer: start dragging, then press Ctrl to copy). Treat it like a
    /// drag that just passed the threshold.
    fn detect_late_modifier(
        &mut self,
        x: i32,
        y: i32,
        modifiers: Modifiers,
        held_button: Button,
    ) -> Option<usize> {
        if self.drag_active
            || self.late_rejected
            || self.paused
            || held_button == Button::None
            || modifiers.is_empty()
        {
            return None;
        }

        let index = self.gestures.iter().position(|g| {
            g.kind == GestureKind::Drag && g.button == held_button && g.modifiers == modifiers
        })?;
        if (self.scope_filter)(&self.gestures[index], x, y) {
            Some(index)
        } else {
            self.late_rejected = true;
            None
        }
    }

    /// A mouse button was released at `(x, y)`.
    ///
    /// The button itself is not consulted, same as Filee: any release ends
    /// the drag. It stays in the signature so the hook glue reads naturally.
    pub fn mouse_up(&mut self, _button: Button, x: i32, y: i32) -> Vec<GestureEvent> {
        self.last_x = x;
        self.last_y = y;
        self.armed = None;
        self.late_rejected = false;
        if std::mem::take(&mut self.drag_active) {
            vec![GestureEvent::DragEnded { x, y }]
        } else {
            Vec::new()
        }
    }

    /// The hold timer for `token` fired.
    pub fn hold_elapsed(&mut self, token: u64) -> Vec<GestureEvent> {
        let Some(armed) = self.armed else {
            return Vec::new();
        };
        let gesture = &self.gestures[armed.gesture];
        if armed.token != token || gesture.kind != GestureKind::Hold {
            return Vec::new();
        }
        self.armed = None;
        vec![GestureEvent::HoldTriggered {
            gesture: gesture.clone(),
            x: armed.x,
            y: armed.y,
        }]
    }

    /// A key went down. `key` is the key name, for example `"Space"`; case
    /// does not matter.
    pub fn key_down(&mut self, key: &str, modifiers: Modifiers) -> Vec<GestureEvent> {
        if self.paused {
            return Vec::new();
        }
        let (x, y) = (self.last_x, self.last_y);
        let Some(gesture) = self.gestures.iter().find(|g| {
            g.kind == GestureKind::KeyChord && g.key_matches(key) && g.modifiers == modifiers
        }) else {
            return Vec::new();
        };
        if !(self.scope_filter)(gesture, x, y) {
            return Vec::new();
        }
        vec![GestureEvent::KeyChordTriggered {
            gesture: gesture.clone(),
            x,
            y,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from Filee's GestureDetectorTests, one test per [Fact] with the
    // same name in snake_case, followed by a few of our own.

    fn create_with(gestures: Vec<TriggerGesture>, filter: ScopeFilter) -> Detector {
        Detector::new(gestures, 4, filter)
    }

    fn create(gestures: Vec<TriggerGesture>) -> Detector {
        create_with(gestures, Box::new(|_, _, _| true))
    }

    fn ctrl_drag() -> TriggerGesture {
        TriggerGesture {
            kind: GestureKind::Drag,
            modifiers: Modifiers::CTRL,
            button: Button::Left,
            ..TriggerGesture::default()
        }
    }

    fn ctrl_alt_space() -> TriggerGesture {
        TriggerGesture {
            kind: GestureKind::KeyChord,
            modifiers: Modifiers::CTRL | Modifiers::ALT,
            button: Button::None,
            key: Some("Space".to_owned()),
            ..TriggerGesture::default()
        }
    }

    fn alt_middle_hold() -> TriggerGesture {
        TriggerGesture {
            kind: GestureKind::Hold,
            modifiers: Modifiers::ALT,
            button: Button::Middle,
            ..TriggerGesture::default()
        }
    }

    fn count_started(events: &[GestureEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, GestureEvent::DragStarted { .. }))
            .count()
    }

    fn count_ended(events: &[GestureEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, GestureEvent::DragEnded { .. }))
            .count()
    }

    fn hold_token(events: &[GestureEvent]) -> Option<u64> {
        events.iter().find_map(|e| match e {
            GestureEvent::RequestHoldTimer { token, .. } => Some(*token),
            _ => None,
        })
    }

    #[test]
    fn drag_fires_after_threshold_with_exact_modifiers() {
        let mut d = create(vec![ctrl_drag()]);
        let mut events = d.mouse_down(Button::Left, 100, 100, Modifiers::CTRL);
        events.extend(d.mouse_move(102, 101, Modifiers::CTRL, Button::None));
        assert_eq!(count_started(&events), 0);
        events.extend(d.mouse_move(110, 100, Modifiers::CTRL, Button::None));
        events.extend(d.mouse_move(150, 100, Modifiers::CTRL, Button::None));
        assert_eq!(count_started(&events), 1);
    }

    #[test]
    fn wrong_modifiers_or_button_do_not_fire() {
        let mut d = create(vec![ctrl_drag()]);
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let mut events = d.mouse_down(Button::Left, 0, 0, ctrl_shift);
        events.extend(d.mouse_move(50, 0, ctrl_shift, Button::None));
        events.extend(d.mouse_up(Button::Left, 50, 0));
        events.extend(d.mouse_down(Button::Right, 0, 0, Modifiers::CTRL));
        events.extend(d.mouse_move(50, 0, Modifiers::CTRL, Button::None));
        assert_eq!(count_started(&events), 0);
    }

    #[test]
    fn modifier_pressed_after_the_drag_started_still_fires_once() {
        let mut d = create(vec![ctrl_drag()]);
        let mut events = d.mouse_down(Button::Left, 0, 0, Modifiers::NONE);
        events.extend(d.mouse_move(30, 0, Modifiers::NONE, Button::Left));
        events.extend(d.mouse_move(40, 0, Modifiers::CTRL, Button::Left));
        events.extend(d.mouse_move(50, 0, Modifiers::CTRL, Button::Left));
        assert_eq!(count_started(&events), 1);
    }

    #[test]
    fn drag_end_is_reported_only_after_a_drag_started() {
        let mut d = create(vec![ctrl_drag()]);
        let mut events = d.mouse_down(Button::Left, 0, 0, Modifiers::CTRL);
        events.extend(d.mouse_up(Button::Left, 0, 0)); // a click, not a drag
        events.extend(d.mouse_down(Button::Left, 0, 0, Modifiers::CTRL));
        events.extend(d.mouse_move(60, 0, Modifiers::CTRL, Button::None));
        events.extend(d.mouse_up(Button::Left, 60, 0));
        assert_eq!(count_ended(&events), 1);
    }

    #[test]
    fn scope_filter_blocks_gestures() {
        let mut d = create_with(vec![ctrl_drag()], Box::new(|_, x, _| x < 1000));
        let mut events = d.mouse_down(Button::Left, 2000, 0, Modifiers::CTRL);
        events.extend(d.mouse_move(2100, 0, Modifiers::CTRL, Button::None));
        assert_eq!(count_started(&events), 0);
    }

    #[test]
    fn paused_detector_ignores_everything() {
        let mut d = create(vec![ctrl_drag(), ctrl_alt_space()]);
        d.set_paused(true);
        let mut events = d.mouse_down(Button::Left, 0, 0, Modifiers::CTRL);
        events.extend(d.mouse_move(80, 0, Modifiers::CTRL, Button::None));
        events.extend(d.key_down("Space", Modifiers::CTRL | Modifiers::ALT));
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn key_chord_uses_last_cursor_position() {
        let mut d = create(vec![ctrl_alt_space()]);
        assert!(
            d.mouse_move(321, 654, Modifiers::NONE, Button::None)
                .is_empty()
        );
        let events = d.key_down("space", Modifiers::CTRL | Modifiers::ALT);
        assert!(matches!(
            events.as_slice(),
            [GestureEvent::KeyChordTriggered { x: 321, y: 654, .. }]
        ));
    }

    #[test]
    fn hold_fires_when_timer_elapses_without_movement() {
        let mut d = create(vec![alt_middle_hold()]);
        let token = hold_token(&d.mouse_down(Button::Middle, 10, 10, Modifiers::ALT));
        let token = token.expect("a hold press asks for a timer");
        let events = d.hold_elapsed(token);
        assert!(matches!(
            events.as_slice(),
            [GestureEvent::HoldTriggered { x: 10, y: 10, .. }]
        ));

        let token = hold_token(&d.mouse_down(Button::Middle, 10, 10, Modifiers::ALT))
            .expect("a hold press asks for a timer");
        // Moving cancels a hold.
        assert!(
            d.mouse_move(60, 10, Modifiers::ALT, Button::Middle)
                .is_empty()
        );
        assert!(d.hold_elapsed(token).is_empty());
    }

    // Our own additions from here on.

    #[test]
    fn landing_exactly_on_the_threshold_is_not_a_move() {
        let mut d = create(vec![ctrl_drag()]);
        d.mouse_down(Button::Left, 0, 0, Modifiers::CTRL);
        assert!(
            d.mouse_move(4, -4, Modifiers::CTRL, Button::Left)
                .is_empty()
        );
        assert_eq!(
            d.mouse_move(5, 0, Modifiers::CTRL, Button::Left),
            vec![GestureEvent::DragStarted {
                gesture: d.gestures()[0].clone(),
                x: 5,
                y: 0
            }]
        );
    }

    #[test]
    fn gesture_threshold_overrides_the_system_default() {
        let mut d = create(vec![TriggerGesture {
            drag_threshold: 20,
            ..ctrl_drag()
        }]);
        d.mouse_down(Button::Left, 0, 0, Modifiers::CTRL);
        assert!(
            d.mouse_move(0, 20, Modifiers::CTRL, Button::Left)
                .is_empty()
        );
        assert_eq!(
            count_started(&d.mouse_move(0, 21, Modifiers::CTRL, Button::Left)),
            1
        );
    }

    #[test]
    fn hold_request_carries_the_configured_delay() {
        let mut d = create(vec![TriggerGesture {
            hold_ms: 700,
            ..alt_middle_hold()
        }]);
        let events = d.mouse_down(Button::Middle, 1, 2, Modifiers::ALT);
        assert!(matches!(
            events.as_slice(),
            [GestureEvent::RequestHoldTimer { ms: 700, .. }]
        ));
    }

    #[test]
    fn stale_hold_token_is_ignored() {
        let mut d = create(vec![alt_middle_hold()]);
        let first = hold_token(&d.mouse_down(Button::Middle, 0, 0, Modifiers::ALT)).unwrap();
        d.mouse_up(Button::Middle, 0, 0);
        let second = hold_token(&d.mouse_down(Button::Middle, 0, 0, Modifiers::ALT)).unwrap();
        assert_ne!(first, second);
        assert!(d.hold_elapsed(first).is_empty());
        assert_eq!(d.hold_elapsed(second).len(), 1);
        // And it only fires once.
        assert!(d.hold_elapsed(second).is_empty());
    }

    #[test]
    fn hold_elapsed_does_not_fire_a_drag_gesture() {
        let mut d = create(vec![ctrl_drag()]);
        d.mouse_down(Button::Left, 0, 0, Modifiers::CTRL);
        // Drag presses get token 1; a bogus timer for it must do nothing.
        assert!(d.hold_elapsed(1).is_empty());
    }

    #[test]
    fn late_modifier_rejected_by_scope_is_not_rechecked() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;

        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let mut d = create_with(
            vec![ctrl_drag()],
            Box::new(move |_, _, _| {
                seen.fetch_add(1, Ordering::Relaxed);
                false
            }),
        );
        d.mouse_down(Button::Left, 0, 0, Modifiers::NONE);
        for x in 10..20 {
            assert!(d.mouse_move(x, 0, Modifiers::CTRL, Button::Left).is_empty());
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        // A fresh press clears the rejection.
        d.mouse_up(Button::Left, 20, 0);
        d.mouse_down(Button::Left, 0, 0, Modifiers::NONE);
        d.mouse_move(30, 0, Modifiers::CTRL, Button::Left);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn late_modifier_needs_a_held_button() {
        let mut d = create(vec![ctrl_drag()]);
        assert!(
            d.mouse_move(10, 0, Modifiers::CTRL, Button::None)
                .is_empty()
        );
        assert!(
            d.mouse_move(10, 0, Modifiers::CTRL, Button::Right)
                .is_empty()
        );
    }

    #[test]
    fn late_drag_ends_on_release() {
        let mut d = create(vec![ctrl_drag()]);
        d.mouse_down(Button::Left, 0, 0, Modifiers::NONE);
        assert_eq!(
            count_started(&d.mouse_move(40, 0, Modifiers::CTRL, Button::Left)),
            1
        );
        assert_eq!(
            d.mouse_up(Button::Left, 42, 7),
            vec![GestureEvent::DragEnded { x: 42, y: 7 }]
        );
    }

    #[test]
    fn key_chord_needs_exact_modifiers_and_scope() {
        let mut d = create_with(vec![ctrl_alt_space()], Box::new(|_, x, _| x >= 0));
        assert!(d.key_down("Space", Modifiers::CTRL).is_empty());
        assert!(
            d.key_down("Space", Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT)
                .is_empty()
        );
        assert!(
            d.key_down("Enter", Modifiers::CTRL | Modifiers::ALT)
                .is_empty()
        );
        d.mouse_move(-5, 0, Modifiers::NONE, Button::None);
        assert!(
            d.key_down("SPACE", Modifiers::CTRL | Modifiers::ALT)
                .is_empty()
        );
    }

    #[test]
    fn disabled_gestures_are_dropped_and_configure_disarms() {
        let mut d = create(vec![TriggerGesture {
            enabled: false,
            ..ctrl_drag()
        }]);
        assert!(d.gestures().is_empty());

        d.configure(vec![ctrl_drag()]);
        d.mouse_down(Button::Left, 0, 0, Modifiers::CTRL);
        d.configure(vec![ctrl_drag()]);
        assert!(
            d.mouse_move(50, 0, Modifiers::CTRL, Button::None)
                .is_empty()
        );
    }

    #[test]
    fn first_matching_gesture_wins() {
        let mut d = create(vec![
            TriggerGesture {
                id: "first".to_owned(),
                ..ctrl_drag()
            },
            TriggerGesture {
                id: "second".to_owned(),
                ..ctrl_drag()
            },
        ]);
        d.mouse_down(Button::Left, 0, 0, Modifiers::CTRL);
        let events = d.mouse_move(50, 0, Modifiers::CTRL, Button::Left);
        assert!(matches!(
            events.as_slice(),
            [GestureEvent::DragStarted { gesture, .. }] if gesture.id == "first"
        ));
    }

    #[test]
    fn defaults_match_filee() {
        let defaults = TriggerGesture::defaults();
        assert_eq!(defaults.len(), 2);
        assert_eq!(defaults[0].kind, GestureKind::Drag);
        assert_eq!(defaults[0].button, Button::Left);
        let expected = if cfg!(target_os = "macos") {
            Modifiers::ALT
        } else {
            Modifiers::CTRL
        };
        assert_eq!(defaults[0].modifiers, expected);
        assert_eq!(defaults[1].kind, GestureKind::KeyChord);
        assert_eq!(defaults[1].modifiers, Modifiers::CTRL | Modifiers::ALT);
        assert_eq!(defaults[1].key.as_deref(), Some("Space"));
        assert!(defaults.iter().all(|g| g.enabled
            && g.hold_ms == DEFAULT_HOLD_MS
            && g.drag_threshold == 0
            && g.scope == Scope::FileManager
            && g.id.len() == 8));
        assert_ne!(defaults[0].id, defaults[1].id);
    }

    #[test]
    fn sanitized_applies_the_settings_limits() {
        let g = TriggerGesture {
            drag_threshold: 999,
            hold_ms: 10,
            ..ctrl_alt_space()
        }
        .sanitized();
        assert_eq!(g.drag_threshold, MAX_DRAG_THRESHOLD);
        assert_eq!(g.hold_ms, MIN_HOLD_MS);
        assert_eq!(g.button, Button::None);

        let g = TriggerGesture {
            hold_ms: 99_999,
            ..alt_middle_hold()
        }
        .sanitized();
        assert_eq!(g.hold_ms, MAX_HOLD_MS);
        assert_eq!(g.button, Button::Middle);
    }

    #[test]
    fn modifiers_behave_like_flags() {
        let both = Modifiers::CTRL | Modifiers::SHIFT;
        assert_eq!(both.bits(), 5);
        assert!(both.contains(Modifiers::CTRL));
        assert!(!both.contains(Modifiers::ALT));
        assert_eq!(both & Modifiers::SHIFT, Modifiers::SHIFT);
        assert!(Modifiers::NONE.is_empty());
        assert_eq!(Modifiers::from_bits_truncate(0xff).bits(), 0b1111);
        let mut m = Modifiers::NONE;
        m |= Modifiers::META;
        assert_eq!(m, Modifiers::META);
        assert_eq!(format!("{both:?}"), "Modifiers(CTRL | SHIFT)");
    }

    #[test]
    fn detector_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Detector>();
    }
}
