//! A tiny Hyprland IPC client: where the cursor is, which window has focus, and
//! "please put the ring over there". Wayland won't tell an app any of this, but
//! Hyprland will, if you ask its socket nicely.
//!
//! Every call returns `None` (or does nothing) when we are not on Hyprland, so
//! callers fall back to a centred ring without a second thought.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// How long one request may take. Hyprland answers in microseconds; anything
/// slower means it is busy, and a ring that shows up late is worse than one
/// that shows up centred.
const TIMEOUT: Duration = Duration::from_millis(250);

fn socket() -> Option<PathBuf> {
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let path = PathBuf::from(runtime)
        .join("hypr")
        .join(signature)
        .join(".socket.sock");
    path.exists().then_some(path)
}

/// Whether this session is a Hyprland one we can talk to.
pub fn available() -> bool {
    socket().is_some()
}

/// Sends one command (as `hyprctl` would) and returns the reply.
fn request(command: &str) -> Option<String> {
    let mut stream = UnixStream::connect(socket()?).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;
    stream.write_all(command.as_bytes()).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    Some(reply)
}

/// The cursor position in global layout coordinates.
pub fn cursor() -> Option<(i32, i32)> {
    let reply: serde_json::Value = serde_json::from_str(&request("j/cursorpos")?).ok()?;
    Some((
        reply.get("x")?.as_i64()? as i32,
        reply.get("y")?.as_i64()? as i32,
    ))
}

/// The class of the focused window, e.g. `org.kde.dolphin`.
pub fn active_class() -> Option<String> {
    let reply: serde_json::Value = serde_json::from_str(&request("j/activewindow")?).ok()?;
    Some(reply.get("class")?.as_str()?.to_owned())
}

/// Moves the (floating) window whose title is exactly `title` so its top-left
/// corner lands on `x`, `y`.
pub fn move_window(title: &str, x: i32, y: i32) {
    // Hyprland 0.56 with a Lua config reads dispatch arguments as Lua, so this
    // is a Lua call, not the old "movewindowpixel exact" string. The title is
    // ours and has no quotes in it, so no escaping heroics are needed.
    let window = format!("title:^({})$", title);
    let command =
        format!("dispatch hl.dsp.window.move({{ x = {x}, y = {y}, window = '{window}' }})");
    match request(&command) {
        Some(reply) if reply.trim() == "ok" => tracing::debug!(x, y, "moved the ring"),
        Some(reply) => {
            // Older Hyprland: the classic dispatcher syntax.
            let legacy = format!("dispatch movewindowpixel exact {x} {y},{window}");
            tracing::debug!(%reply, "Lua move failed; trying the classic dispatcher");
            let _ = request(&legacy);
        }
        None => tracing::debug!("Hyprland did not answer the move request"),
    }
}
