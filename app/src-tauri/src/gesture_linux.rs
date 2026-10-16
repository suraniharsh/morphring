//! The Ctrl+drag watcher for Linux, reading keyboards and mice straight from
//! evdev. Wayland gives apps no global input, on purpose; evdev does, to users
//! in the `input` group. Without that access this quietly does nothing and the
//! main window's drop zone stays the way in.
//!
//! The gesture is Filee's default: hold Ctrl and the left button (either one
//! first), move far enough to be a drag, and the ring opens under the cursor.
//! Only drags that start in a file manager count, so Ctrl+dragging text in a
//! browser doesn't summon a donut.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use evdev::{Device, EventSummary, KeyCode};
use tauri::{AppHandle, Runtime};

/// How far the cursor moves before a press becomes a drag, in pixels.
/// File managers start their own drag at a few pixels; ours waits a little
/// longer so a Ctrl+click to select never pops the ring.
const DRAG_THRESHOLD: i32 = 12;
/// How often the cursor is polled while armed.
const POLL: Duration = Duration::from_millis(20);
/// An armed press that never turns into a drag stops polling after this long.
const ARM_TIMEOUT: Duration = Duration::from_secs(15);

/// Window classes of file managers, lowercased. Matched as substrings, so
/// `org.kde.dolphin` and `dolphin` both count.
const FILE_MANAGERS: &[&str] = &[
    "dolphin",
    "nautilus",
    "nemo",
    "thunar",
    "pcmanfm",
    "caja",
    "krusader",
    "doublecmd",
    "spacefm",
    "files",
    "konqueror",
];

#[derive(Debug, Default)]
struct State {
    left_ctrl: bool,
    right_ctrl: bool,
    button: bool,
    phase: Phase,
    /// Bumped on every arm, so a stale poller knows to stop.
    generation: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Armed {
        x: i32,
        y: i32,
    },
    Fired,
    /// Ctrl+button went down outside a file manager; ignore until released.
    Rejected,
}

impl State {
    fn ctrl(&self) -> bool {
        self.left_ctrl || self.right_ctrl
    }
}

/// What a key or button change asks the watcher to do.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    None,
    Arm,
    Released,
}

/// Applies one key change to the state. Pure, so it can be tested without a
/// keyboard, a mouse or a compositor.
fn on_key(state: &mut State, code: KeyCode, down: bool) -> Action {
    match code {
        KeyCode::KEY_LEFTCTRL => state.left_ctrl = down,
        KeyCode::KEY_RIGHTCTRL => state.right_ctrl = down,
        KeyCode::BTN_LEFT => state.button = down,
        _ => return Action::None,
    }
    if code == KeyCode::BTN_LEFT && !down {
        // The button decides when a drag ends, not Ctrl: people let go of
        // Ctrl mid-drag all the time, and the drop is what counts.
        let fired = state.phase == Phase::Fired;
        state.phase = Phase::Idle;
        return if fired {
            Action::Released
        } else {
            Action::None
        };
    }
    match state.phase {
        Phase::Idle if state.ctrl() && state.button => Action::Arm,
        // Letting go of Ctrl before moving means it was a Ctrl+click.
        Phase::Armed { .. } if !state.ctrl() => {
            state.phase = Phase::Idle;
            Action::None
        }
        _ => Action::None,
    }
}

fn in_file_manager() -> bool {
    if std::env::var_os("MORPHRING_GESTURE_ANYWHERE").is_some() {
        return true;
    }
    match crate::hyprland::active_class() {
        Some(class) => {
            let class = class.to_lowercase();
            FILE_MANAGERS.iter().any(|fm| class.contains(fm))
        }
        // No way to ask: better a ring too many than a gesture that never works.
        None => true,
    }
}

/// Starts watching every keyboard and mouse. Returns false when no input
/// device could be opened (no `input` group) or the user turned it off.
pub fn start<R: Runtime>(app: &AppHandle<R>) -> bool {
    if std::env::var_os("MORPHRING_NO_GESTURE").is_some() {
        return false;
    }
    let devices: Vec<Device> = evdev::enumerate()
        .map(|(_, device)| device)
        .filter(|device| {
            device.supported_keys().is_some_and(|keys| {
                keys.contains(KeyCode::KEY_LEFTCTRL) || keys.contains(KeyCode::BTN_LEFT)
            })
        })
        .collect();
    if devices.is_empty() {
        tracing::info!(
            "no readable input devices; Ctrl+drag is off (join the input group to turn it on)"
        );
        return false;
    }
    tracing::info!(devices = devices.len(), "Ctrl+drag watcher started");
    let state = Arc::new(Mutex::new(State::default()));
    for mut device in devices {
        let state = Arc::clone(&state);
        let app = app.clone();
        std::thread::spawn(move || {
            loop {
                let events = match device.fetch_events() {
                    Ok(events) => events.collect::<Vec<_>>(),
                    // Unplugged, most likely. One fewer mouse to listen to.
                    Err(_) => return,
                };
                for event in events {
                    // value 2 is key repeat; Ctrl held down repeats happily.
                    if let EventSummary::Key(_, code, value @ (0 | 1)) = event.destructure() {
                        handle(&app, &state, code, value == 1);
                    }
                }
            }
        });
    }
    true
}

fn handle<R: Runtime>(app: &AppHandle<R>, state: &Arc<Mutex<State>>, code: KeyCode, down: bool) {
    let action = {
        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
        on_key(&mut s, code, down)
    };
    match action {
        Action::None => {}
        Action::Released => crate::drag_ring::released(app),
        Action::Arm => {
            // Ask Hyprland outside the lock; it is quick, but not free.
            let ok = in_file_manager();
            let cursor = crate::hyprland::cursor();
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if s.phase != Phase::Idle || !(s.ctrl() && s.button) {
                return;
            }
            if !ok {
                s.phase = Phase::Rejected;
                return;
            }
            let (x, y) = cursor.unwrap_or((0, 0));
            s.phase = Phase::Armed { x, y };
            s.generation += 1;
            let generation = s.generation;
            drop(s);
            watch_for_drag(app.clone(), Arc::clone(state), generation, cursor.is_some());
        }
    }
}

/// Polls the cursor until the press becomes a drag (then opens the ring) or
/// stops being a candidate.
fn watch_for_drag<R: Runtime>(
    app: AppHandle<R>,
    state: Arc<Mutex<State>>,
    generation: u64,
    know_cursor: bool,
) {
    std::thread::spawn(move || {
        let started = Instant::now();
        loop {
            std::thread::sleep(POLL);
            let now = if know_cursor {
                crate::hyprland::cursor()
            } else {
                None
            };
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            let Phase::Armed { x, y } = s.phase else {
                return;
            };
            if s.generation != generation || started.elapsed() > ARM_TIMEOUT {
                return;
            }
            let moved = match now {
                Some((nx, ny)) => (nx - x).abs().max((ny - y).abs()) >= DRAG_THRESHOLD,
                // Can't see the cursor: a press held this long is a drag.
                None => started.elapsed() >= Duration::from_millis(250),
            };
            if moved {
                s.phase = Phase::Fired;
                drop(s);
                crate::drag_ring::arm(&app, now);
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(s: &mut State, code: KeyCode) -> Action {
        on_key(s, code, true)
    }
    fn release(s: &mut State, code: KeyCode) -> Action {
        on_key(s, code, false)
    }

    #[test]
    fn ctrl_then_button_arms() {
        let mut s = State::default();
        assert_eq!(press(&mut s, KeyCode::KEY_LEFTCTRL), Action::None);
        assert_eq!(press(&mut s, KeyCode::BTN_LEFT), Action::Arm);
    }

    #[test]
    fn button_then_ctrl_arms_too() {
        // Filee's "late modifier": start dragging, then remember Ctrl.
        let mut s = State::default();
        assert_eq!(press(&mut s, KeyCode::BTN_LEFT), Action::None);
        assert_eq!(press(&mut s, KeyCode::KEY_RIGHTCTRL), Action::Arm);
    }

    #[test]
    fn button_alone_never_arms() {
        let mut s = State::default();
        assert_eq!(press(&mut s, KeyCode::BTN_LEFT), Action::None);
        assert_eq!(release(&mut s, KeyCode::BTN_LEFT), Action::None);
    }

    #[test]
    fn letting_go_of_ctrl_before_moving_disarms() {
        let mut s = State::default();
        press(&mut s, KeyCode::KEY_LEFTCTRL);
        press(&mut s, KeyCode::BTN_LEFT);
        s.phase = Phase::Armed { x: 0, y: 0 };
        release(&mut s, KeyCode::KEY_LEFTCTRL);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn releasing_the_button_after_firing_reports_the_release() {
        let mut s = State::default();
        press(&mut s, KeyCode::KEY_LEFTCTRL);
        press(&mut s, KeyCode::BTN_LEFT);
        s.phase = Phase::Fired;
        // Ctrl may go first; the ring stays until the button decides.
        assert_eq!(release(&mut s, KeyCode::KEY_LEFTCTRL), Action::None);
        assert_eq!(s.phase, Phase::Fired);
        assert_eq!(release(&mut s, KeyCode::BTN_LEFT), Action::Released);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn a_rejected_press_stays_rejected_until_the_button_comes_up() {
        let mut s = State::default();
        press(&mut s, KeyCode::KEY_LEFTCTRL);
        press(&mut s, KeyCode::BTN_LEFT);
        s.phase = Phase::Rejected;
        assert_eq!(press(&mut s, KeyCode::KEY_RIGHTCTRL), Action::None);
        assert_eq!(release(&mut s, KeyCode::BTN_LEFT), Action::None);
        assert_eq!(s.phase, Phase::Idle);
    }

    #[test]
    fn other_keys_are_ignored() {
        let mut s = State::default();
        press(&mut s, KeyCode::KEY_LEFTCTRL);
        assert_eq!(press(&mut s, KeyCode::KEY_A), Action::None);
        assert_eq!(s.phase, Phase::Idle);
    }
}
