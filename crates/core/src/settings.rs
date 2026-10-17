//! The settings store: app settings and the atomic JSON files they live in.
//! Ported from Filee's `src/Filee.Core/Settings/AppSettings.cs` and
//! `UserDataStore.cs`.
//!
//! The rules (ENGINEERING.md section 7): camelCase JSON, `#[serde(default)]`
//! everywhere, atomic writes, and a corrupt file never crashes anything. It
//! gets copied to `<name>.broken` and we carry on with defaults, like adults.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tempfile::NamedTempFile;

use crate::output::{DEFAULT_SUBFOLDER, OutputLocation, OutputRule};

/// File name of the app settings inside the store dir.
pub const SETTINGS_FILE: &str = "settings.json";

/// Current `schemaVersion` of `settings.json`.
pub const SETTINGS_SCHEMA_VERSION: u32 = 2;

/// A folder of JSON files. `dir: None` means "no config dir at all": reads
/// give defaults and writes are quietly skipped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Store {
    pub dir: Option<PathBuf>,
}

impl Store {
    /// A store over `dir`, or a store that never touches the disk.
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    /// Reads `name` from the store dir. A missing file (or no dir) gives the
    /// default. A file that can't be read or parsed is copied to
    /// `<name>.broken`, logged, and also gives the default.
    pub fn read_json<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        let Some(dir) = &self.dir else {
            return T::default();
        };
        let path = dir.join(name);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return T::default(),
            Err(e) => {
                quarantine(&path, &e);
                return T::default();
            }
        };
        match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(e) => {
                quarantine(&path, &e);
                T::default()
            }
        }
    }

    /// Writes `value` to `name` atomically: a temp file in the same dir,
    /// pretty JSON, `sync_all`, then a rename over the old file. With no dir
    /// it writes nothing and returns `Ok`.
    pub fn write_json<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        fs::create_dir_all(dir)?;
        let mut tmp = NamedTempFile::new_in(dir)?;
        serde_json::to_writer_pretty(&mut tmp, value)?;
        tmp.write_all(b"\n")?;
        tmp.as_file().sync_all()?;
        tmp.persist(dir.join(name)).map_err(|e| e.error)?;
        Ok(())
    }

    /// Loads `settings.json`.
    pub fn load_settings(&self) -> AppSettings {
        self.read_json(SETTINGS_FILE)
    }

    /// Saves `settings.json`.
    pub fn save_settings(&self, settings: &AppSettings) -> io::Result<()> {
        self.write_json(SETTINGS_FILE, settings)
    }
}

/// Copies a file we could not read to `<name>.broken` so the next save does
/// not destroy the evidence. Best effort: if even the copy fails, we shrug
/// in the log and move on.
fn quarantine(path: &Path, err: &dyn fmt::Display) {
    let mut broken = path.as_os_str().to_owned();
    broken.push(".broken");
    let broken = PathBuf::from(broken);
    match fs::copy(path, &broken) {
        Ok(_) => tracing::warn!(
            path = %path.display(),
            copy = %broken.display(),
            error = %err,
            "could not read the file; using defaults"
        ),
        Err(copy_err) => tracing::warn!(
            path = %path.display(),
            error = %err,
            copy_error = %copy_err,
            "could not read the file and could not back it up; using defaults"
        ),
    }
}

/// App settings: everything Filee keeps in `settings.json` except presets,
/// profiles and watch folders, which live elsewhere. Anything unknown in the
/// file is ignored, anything missing takes its default, and numbers out of
/// range are clamped on the way in (see [`RawAppSettings`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "RawAppSettings")]
pub struct AppSettings {
    pub schema_version: u32,
    /// Interface language: `auto`, `en`, `ko` or `zh-CN`.
    pub language: String,
    /// Start MorphRing when the user logs in.
    pub start_with_system: bool,
    /// Offer "Convert with MorphRing" in the file manager's context menu.
    pub context_menu_enabled: bool,
    /// Ask the internet about new releases. Off unless the user opts in:
    /// MorphRing is offline by default (ENGINEERING.md 1.5), unlike Filee.
    pub check_for_updates: bool,
    /// Record finished conversions in the history.
    pub keep_history: bool,
    /// Give every output the dates of its source, whatever the preset says.
    pub keep_file_dates: bool,
    /// Every trigger gesture is off for now (the tray's "Pause").
    pub paused: bool,
    /// Where presets set to [`OutputLocation::Default`] write.
    pub default_output: DefaultOutput,
    pub theme: ThemeSettings,
    pub donut: DonutSettings,
    /// The gestures that open the ring.
    pub triggers: Vec<TriggerGesture>,
    /// Process names (no `.exe`) in which no gesture ever fires.
    pub excluded_processes: Vec<String>,
    /// Engine ids, most preferred first. Empty means registry order.
    pub engine_priority: Vec<String>,
    /// Which document kinds go through LibreOffice when it is installed.
    pub libre_office_for: LibreOfficeFor,
    /// The first-run welcome has been shown.
    pub first_run_completed: bool,
    /// Fully successful conversions, counted until the feedback card shows.
    pub successful_conversions: u32,
    /// The "is MorphRing helping you?" card has been shown.
    pub feedback_prompt_shown: bool,
    /// Engine id -> the program the user already has, used instead of ours.
    pub engine_own_copies: BTreeMap<String, PathBuf>,
    /// Newest release the user was already told about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notified_update_version: Option<String>,
    /// Engine id -> the version its update notice was shown for.
    pub notified_engine_versions: BTreeMap<String, String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            language: "auto".to_owned(),
            start_with_system: true,
            context_menu_enabled: true,
            check_for_updates: false,
            keep_history: true,
            keep_file_dates: false,
            paused: false,
            default_output: DefaultOutput::default(),
            theme: ThemeSettings::default(),
            donut: DonutSettings::default(),
            triggers: TriggerGesture::defaults(),
            excluded_processes: Vec::new(),
            engine_priority: Vec::new(),
            libre_office_for: LibreOfficeFor::default(),
            first_run_completed: false,
            successful_conversions: 0,
            feedback_prompt_shown: false,
            engine_own_copies: BTreeMap::new(),
            notified_update_version: None,
            notified_engine_versions: BTreeMap::new(),
        }
    }
}

/// The languages the interface speaks. Anything else reads as `auto`.
pub const LANGUAGES: [&str; 4] = ["auto", "en", "ko", "zh-CN"];

/// `settings.json` as it may arrive: ours, an older one of ours, or one with
/// Filee's spellings. Everything optional is optional so we can tell "missing"
/// from "false" and fill in the right default. It exists only to be turned
/// into an [`AppSettings`], which is where the clamping happens.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawAppSettings {
    schema_version: Option<u32>,
    language: Option<String>,
    start_with_system: Option<bool>,
    context_menu_enabled: Option<bool>,
    check_for_updates: Option<bool>,
    keep_history: Option<bool>,
    keep_file_dates: bool,
    paused: bool,
    default_output: DefaultOutput,
    theme: ThemeSettings,
    donut: DonutSettings,
    triggers: Option<Vec<TriggerGesture>>,
    excluded_processes: Vec<String>,
    engine_priority: Vec<String>,
    libre_office_for: Option<LibreOfficeFor>,
    // Filee keeps these four flat at the top level; we nest them.
    libre_office_for_slides: Option<bool>,
    libre_office_for_hwp: Option<bool>,
    libre_office_for_documents: Option<bool>,
    libre_office_for_sheets: Option<bool>,
    first_run_completed: bool,
    successful_conversions: Option<f64>,
    feedback_prompt_shown: bool,
    engine_own_copies: BTreeMap<String, PathBuf>,
    notified_update_version: Option<String>,
    notified_engine_versions: BTreeMap<String, String>,
}

impl From<RawAppSettings> for AppSettings {
    fn from(raw: RawAppSettings) -> Self {
        let d = AppSettings::default();
        // A file from an older schema has had its "migration" the moment
        // serde filled in the new fields, so it is current now. A file from
        // the future keeps its number; we don't pretend to know better.
        let schema_version = raw
            .schema_version
            .map_or(SETTINGS_SCHEMA_VERSION, |v| v.max(SETTINGS_SCHEMA_VERSION));
        let language = raw
            .language
            .and_then(|l| {
                LANGUAGES
                    .iter()
                    .find(|known| known.eq_ignore_ascii_case(l.trim()))
                    .map(|known| (*known).to_owned())
            })
            .unwrap_or(d.language);
        let mut libre_office_for = raw.libre_office_for.unwrap_or_default();
        let flat = [
            (raw.libre_office_for_slides, &mut libre_office_for.slides),
            (raw.libre_office_for_hwp, &mut libre_office_for.hwp),
            (raw.libre_office_for_documents, &mut libre_office_for.word),
            (raw.libre_office_for_sheets, &mut libre_office_for.sheets),
        ];
        for (value, slot) in flat {
            if let Some(value) = value {
                *slot = value;
            }
        }
        let excluded_processes = raw
            .excluded_processes
            .into_iter()
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty())
            .collect();
        Self {
            schema_version,
            language,
            start_with_system: raw.start_with_system.unwrap_or(d.start_with_system),
            context_menu_enabled: raw.context_menu_enabled.unwrap_or(d.context_menu_enabled),
            check_for_updates: raw.check_for_updates.unwrap_or(d.check_for_updates),
            keep_history: raw.keep_history.unwrap_or(d.keep_history),
            keep_file_dates: raw.keep_file_dates,
            paused: raw.paused,
            default_output: raw.default_output,
            theme: raw.theme,
            donut: raw.donut,
            triggers: raw.triggers.unwrap_or(d.triggers),
            excluded_processes,
            engine_priority: raw.engine_priority,
            libre_office_for,
            first_run_completed: raw.first_run_completed,
            successful_conversions: clamp_int(raw.successful_conversions, 0, u32::MAX, 0),
            feedback_prompt_shown: raw.feedback_prompt_shown,
            engine_own_copies: raw.engine_own_copies,
            notified_update_version: raw.notified_update_version,
            notified_engine_versions: raw.notified_engine_versions,
        }
    }
}

/// `value` clamped to `lo..=hi`, or `default` when it is missing or not a
/// finite number (JSON can't say NaN, but belt and braces).
fn clamp_f64(value: Option<f64>, lo: f64, hi: f64, default: f64) -> f64 {
    match value {
        Some(v) if v.is_finite() => v.clamp(lo, hi),
        _ => default,
    }
}

/// [`clamp_f64`] for whole numbers. Filee writes ints, but a hand-edited
/// `450.7` rounds instead of declaring the whole file broken.
fn clamp_int(value: Option<f64>, lo: u32, hi: u32, default: u32) -> u32 {
    match value {
        // The clamp keeps the cast in range, so `as` truncates nothing.
        Some(v) if v.is_finite() => v.round().clamp(f64::from(lo), f64::from(hi)) as u32,
        _ => default,
    }
}

/// Defines a unit enum that writes its variant name and reads any spelling
/// of it (case-insensitive, plus Filee's names as aliases). Something
/// unknown becomes the default rather than breaking the whole file.
macro_rules! lenient_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident $( = [$($alias:literal),*] )? ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The name written to JSON.
            pub fn as_str(self) -> &'static str {
                match self {
                    $( $name::$variant => stringify!($variant) ),+
                }
            }

            fn parse(raw: &str) -> Option<Self> {
                let raw = raw.trim();
                $(
                    if stringify!($variant).eq_ignore_ascii_case(raw)
                        $( $( || $alias.eq_ignore_ascii_case(raw) )* )?
                    {
                        return Some($name::$variant);
                    }
                )+
                None
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(d)?;
                Ok(Self::parse(&raw).unwrap_or_default())
            }
        }
    };
}

lenient_enum! {
    /// Light, dark, or whatever the system says.
    pub enum ThemeMode {
        #[default]
        System,
        Light,
        Dark,
    }
}

lenient_enum! {
    /// Animations: follow the system, always reduce, or never reduce.
    /// Filee stores this as a nullable bool; see [`ThemeSettings`].
    pub enum ReduceMotion {
        #[default]
        Auto,
        On = ["true"],
        Off = ["false"],
    }
}

/// Default accent colour, Filee's lavender.
pub const DEFAULT_ACCENT: &str = "#7F77DD";

/// Look and feel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "RawTheme")]
pub struct ThemeSettings {
    pub mode: ThemeMode,
    /// `#RRGGBB`. Anything else reads as [`DEFAULT_ACCENT`].
    pub accent: String,
    /// Base corner radius in logical pixels, 0-28.
    pub corner_radius: f64,
    pub reduce_motion: ReduceMotion,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            mode: ThemeMode::System,
            accent: DEFAULT_ACCENT.to_owned(),
            corner_radius: 16.0,
            reduce_motion: ReduceMotion::Auto,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawTheme {
    mode: ThemeMode,
    accent: Option<String>,
    corner_radius: Option<f64>,
    reduce_motion: Option<RawReduceMotion>,
}

/// Filee writes `true`/`false` (or leaves it out for "follow the system");
/// we write `On`/`Off`/`Auto`. Both read fine.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawReduceMotion {
    Bool(bool),
    Named(ReduceMotion),
}

impl From<RawTheme> for ThemeSettings {
    fn from(raw: RawTheme) -> Self {
        let d = ThemeSettings::default();
        let accent = raw
            .accent
            .map(|a| a.trim().to_ascii_uppercase())
            .filter(|a| is_hex_colour(a))
            .unwrap_or(d.accent);
        let reduce_motion = match raw.reduce_motion {
            None => ReduceMotion::Auto,
            Some(RawReduceMotion::Bool(true)) => ReduceMotion::On,
            Some(RawReduceMotion::Bool(false)) => ReduceMotion::Off,
            Some(RawReduceMotion::Named(m)) => m,
        };
        Self {
            mode: raw.mode,
            accent,
            corner_radius: clamp_f64(raw.corner_radius, 0.0, 28.0, d.corner_radius),
            reduce_motion,
        }
    }
}

/// `#RRGGBB`, nothing more, nothing less (no `#RGB`, no alpha).
fn is_hex_colour(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Size and look of the ring.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "RawDonut")]
pub struct DonutSettings {
    /// Outer radius in logical pixels, 110-220.
    pub outer_radius: f64,
    /// Hole radius as a fraction of the outer radius, 0.25-0.65. The ring
    /// renderer clamps again to 0.2-0.75, so a hand-edited file can't make
    /// the hole swallow the slices either way.
    pub hole_ratio: f64,
    /// Slice background opacity, 0.5-1.
    pub opacity: f64,
}

impl Default for DonutSettings {
    fn default() -> Self {
        Self {
            outer_radius: 150.0,
            hole_ratio: 0.42,
            opacity: 0.96,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawDonut {
    outer_radius: Option<f64>,
    hole_ratio: Option<f64>,
    opacity: Option<f64>,
}

impl From<RawDonut> for DonutSettings {
    fn from(raw: RawDonut) -> Self {
        let d = DonutSettings::default();
        Self {
            outer_radius: clamp_f64(raw.outer_radius, 110.0, 220.0, d.outer_radius),
            hole_ratio: clamp_f64(raw.hole_ratio, 0.25, 0.65, d.hole_ratio),
            opacity: clamp_f64(raw.opacity, 0.5, 1.0, d.opacity),
        }
    }
}

/// Which document kinds go to LibreOffice instead of the built-in route,
/// when LibreOffice is installed. Filee's defaults: only slides.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibreOfficeFor {
    /// PPTX, PPT, ODP -> PDF.
    pub slides: bool,
    /// HWP, HWPX -> PDF (needs H2Orestart).
    pub hwp: bool,
    /// DOCX, DOC, ODT, RTF -> PDF. Filee calls this one "documents".
    #[serde(alias = "documents")]
    pub word: bool,
    /// XLSX, XLS, ODS, CSV -> PDF.
    pub sheets: bool,
}

impl Default for LibreOfficeFor {
    fn default() -> Self {
        Self {
            slides: true,
            hwp: false,
            word: false,
            sheets: false,
        }
    }
}

lenient_enum! {
    /// What completes a gesture.
    pub enum TriggerKind {
        /// Hold the modifiers and drag past the threshold.
        #[default]
        Drag,
        /// Hold the modifiers and keep the button down without moving.
        Hold,
        /// Modifiers plus a key, no mouse. Uses the file manager selection,
        /// which Linux has no API for, so it stays inert there.
        KeyChord,
    }
}

lenient_enum! {
    /// The mouse button a gesture uses.
    pub enum TriggerButton {
        None,
        #[default]
        Left,
        Right,
        Middle,
        X1,
        X2,
    }
}

lenient_enum! {
    /// Where a gesture is active.
    pub enum TriggerScope {
        /// Over the file manager or the desktop only.
        #[default]
        FileManagers = ["FileManager"],
        /// Everywhere except the excluded apps.
        Everywhere = ["Anywhere"],
    }
}

/// Default hold time for [`TriggerKind::Hold`], in milliseconds.
pub const DEFAULT_HOLD_MS: u32 = 450;

/// A gesture that opens the ring.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "RawTrigger")]
pub struct TriggerGesture {
    pub kind: TriggerKind,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Windows key on Windows, Command on macOS, Super elsewhere.
    pub meta: bool,
    pub button: TriggerButton,
    /// The extra non-modifier key, by name (`"Space"`). Needed for a key
    /// chord, optional otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Logical pixels a drag travels before it counts, 0-200. 0 means the
    /// system's own value.
    pub drag_distance: u32,
    /// How long a hold lasts, 150-3000 ms.
    pub hold_ms: u32,
    pub scope: TriggerScope,
    pub enabled: bool,
}

impl Default for TriggerGesture {
    /// Filee's blank gesture: Ctrl + left drag over the file manager.
    fn default() -> Self {
        Self {
            kind: TriggerKind::Drag,
            ctrl: true,
            alt: false,
            shift: false,
            meta: false,
            button: TriggerButton::Left,
            key: None,
            drag_distance: 0,
            hold_ms: DEFAULT_HOLD_MS,
            scope: TriggerScope::FileManagers,
            enabled: true,
        }
    }
}

impl TriggerGesture {
    /// Filee's default gestures for this OS: a modifier + left drag, and the
    /// key chord Ctrl+Alt+Space. On macOS the drag uses Alt, because
    /// Ctrl+click is a right click there and nobody wants that argument.
    pub fn defaults() -> Vec<TriggerGesture> {
        let mac = cfg!(target_os = "macos");
        vec![
            TriggerGesture {
                kind: TriggerKind::Drag,
                ctrl: !mac,
                alt: mac,
                button: TriggerButton::Left,
                ..TriggerGesture::default()
            },
            TriggerGesture {
                kind: TriggerKind::KeyChord,
                ctrl: true,
                alt: true,
                button: TriggerButton::None,
                key: Some("Space".to_owned()),
                ..TriggerGesture::default()
            },
        ]
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RawTrigger {
    kind: TriggerKind,
    ctrl: Option<bool>,
    alt: Option<bool>,
    shift: Option<bool>,
    meta: Option<bool>,
    /// Filee's `[Flags]` enum: `"Ctrl, Alt"` as a string, or the number.
    modifiers: Option<RawModifiers>,
    button: Option<TriggerButton>,
    key: Option<String>,
    #[serde(alias = "dragThreshold")]
    drag_distance: Option<f64>,
    #[serde(alias = "holdMilliseconds")]
    hold_ms: Option<f64>,
    scope: TriggerScope,
    enabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawModifiers {
    Bits(u64),
    Names(String),
}

impl RawModifiers {
    /// Ctrl, Alt, Shift, Meta, in Filee's bit order (1, 2, 4, 8).
    fn flags(&self) -> [bool; 4] {
        match self {
            RawModifiers::Bits(b) => [b & 1 != 0, b & 2 != 0, b & 4 != 0, b & 8 != 0],
            RawModifiers::Names(s) => {
                let mut out = [false; 4];
                for part in s.split([',', '|', '+']) {
                    let i = ["ctrl", "alt", "shift", "meta"]
                        .iter()
                        .position(|n| n.eq_ignore_ascii_case(part.trim()));
                    if let Some(i) = i {
                        out[i] = true;
                    }
                }
                out
            }
        }
    }
}

impl From<RawTrigger> for TriggerGesture {
    fn from(raw: RawTrigger) -> Self {
        let d = TriggerGesture::default();
        // Explicit booleans win over Filee's flags; with neither, the
        // default (Ctrl alone) stands.
        let [f_ctrl, f_alt, f_shift, f_meta] = match &raw.modifiers {
            Some(m) => m.flags(),
            None => [d.ctrl, d.alt, d.shift, d.meta],
        };
        let key = raw
            .key
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty());
        Self {
            kind: raw.kind,
            ctrl: raw.ctrl.unwrap_or(f_ctrl),
            alt: raw.alt.unwrap_or(f_alt),
            shift: raw.shift.unwrap_or(f_shift),
            meta: raw.meta.unwrap_or(f_meta),
            button: raw.button.unwrap_or(d.button),
            key,
            drag_distance: clamp_int(raw.drag_distance, 0, 200, d.drag_distance),
            hold_ms: clamp_int(raw.hold_ms, 150, 3000, d.hold_ms),
            scope: raw.scope,
            enabled: raw.enabled.unwrap_or(d.enabled),
        }
    }
}

impl AppSettings {
    /// Filee's `WithDefaultOutput`: swaps [`OutputLocation::Default`] for the
    /// default output location and turns on `keep_dates` when
    /// `keep_file_dates` is set. Any other location is left alone.
    pub fn with_default_output(&self, rule: &OutputRule) -> OutputRule {
        let mut out = rule.clone();
        if out.location == OutputLocation::Default {
            out.location = self.default_output.to_location();
        }
        out.keep_dates |= self.keep_file_dates;
        out
    }
}

/// The save location presets set to "Default" use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DefaultOutput {
    pub location: DefaultLocation,
    /// Used when `location` is [`DefaultLocation::Subfolder`].
    pub subfolder_name: String,
    /// Used when `location` is [`DefaultLocation::CustomFolder`].
    pub custom_folder: String,
}

impl Default for DefaultOutput {
    fn default() -> Self {
        Self {
            location: DefaultLocation::SameFolder,
            subfolder_name: DEFAULT_SUBFOLDER.to_owned(),
            custom_folder: String::new(),
        }
    }
}

impl DefaultOutput {
    /// The [`OutputLocation`] this default stands for. Blank names are fine
    /// here: `output::resolve` already falls back to the source folder for an
    /// empty custom folder and to "converted" for an empty subfolder.
    pub fn to_location(&self) -> OutputLocation {
        match self.location {
            DefaultLocation::SameFolder => OutputLocation::SameFolder,
            DefaultLocation::Subfolder => OutputLocation::Subfolder(self.subfolder_name.clone()),
            DefaultLocation::CustomFolder => OutputLocation::CustomFolder {
                path: PathBuf::from(&self.custom_folder),
                keep_folders_from: None,
            },
        }
    }
}

/// The kind of default output location. Read case-insensitively; a value we
/// don't know becomes `SameFolder` instead of declaring the whole file broken
/// over one typo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DefaultLocation {
    #[default]
    SameFolder,
    Subfolder,
    CustomFolder,
}

impl DefaultLocation {
    fn as_str(self) -> &'static str {
        match self {
            Self::SameFolder => "SameFolder",
            Self::Subfolder => "Subfolder",
            Self::CustomFolder => "CustomFolder",
        }
    }
}

impl Serialize for DefaultLocation {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DefaultLocation {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        let found = [Self::SameFolder, Self::Subfolder, Self::CustomFolder]
            .into_iter()
            .find(|l| l.as_str().eq_ignore_ascii_case(raw.trim()));
        Ok(found.unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store(tmp: &TempDir) -> Store {
        Store::new(Some(tmp.path().to_path_buf()))
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn missing_file_gives_defaults() {
        let tmp = TempDir::new().unwrap();
        let s = store(&tmp).load_settings();
        assert_eq!(s, AppSettings::default());
        assert_eq!(s.schema_version, 2);
        assert!(s.keep_history);
        assert!(!s.keep_file_dates);
        assert_eq!(s.default_output.location, DefaultLocation::SameFolder);
        assert_eq!(s.default_output.subfolder_name, "converted");
        assert_eq!(s.default_output.custom_folder, "");
    }

    #[test]
    fn missing_dir_gives_defaults() {
        let tmp = TempDir::new().unwrap();
        let store = Store::new(Some(tmp.path().join("nope")));
        assert_eq!(store.load_settings(), AppSettings::default());
    }

    #[test]
    fn round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = store(&tmp);
        let settings = AppSettings {
            keep_history: false,
            keep_file_dates: true,
            default_output: DefaultOutput {
                location: DefaultLocation::CustomFolder,
                subfolder_name: "out".into(),
                custom_folder: "/srv/out".into(),
            },
            ..AppSettings::default()
        };
        store.save_settings(&settings).unwrap();
        assert_eq!(store.load_settings(), settings);

        let text = fs::read_to_string(tmp.path().join(SETTINGS_FILE)).unwrap();
        assert!(text.contains("\"schemaVersion\": 2"), "{text}");
        assert!(text.contains("\"defaultOutput\""), "{text}");
        assert!(text.contains("\"CustomFolder\""), "{text}");
    }

    #[test]
    fn write_creates_the_dir() {
        let tmp = TempDir::new().unwrap();
        let store = Store::new(Some(tmp.path().join("a/b")));
        store.save_settings(&AppSettings::default()).unwrap();
        assert!(tmp.path().join("a/b").join(SETTINGS_FILE).is_file());
    }

    #[test]
    fn unknown_fields_ignored_and_missing_fields_defaulted() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join(SETTINGS_FILE),
            r##"{"keepFileDates": true, "theme": {"accent": "#7F77DD"},
                "defaultOutput": {"location": "subfolder"}}"##,
        )
        .unwrap();
        let s = store(&tmp).load_settings();
        assert!(s.keep_file_dates);
        assert!(s.keep_history);
        assert_eq!(s.default_output.location, DefaultLocation::Subfolder);
        assert_eq!(s.default_output.subfolder_name, "converted");
        assert!(!tmp.path().join("settings.json.broken").exists());
    }

    #[test]
    fn unknown_location_falls_back_to_same_folder() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join(SETTINGS_FILE),
            r#"{"defaultOutput": {"location": "Moon"}}"#,
        )
        .unwrap();
        let s = store(&tmp).load_settings();
        assert_eq!(s.default_output.location, DefaultLocation::SameFolder);
    }

    #[test]
    fn corrupt_file_gives_defaults_and_a_broken_copy() {
        let tmp = TempDir::new().unwrap();
        let bad = "{ \"keepHistory\": fals";
        fs::write(tmp.path().join(SETTINGS_FILE), bad).unwrap();
        let s = store(&tmp).load_settings();
        assert_eq!(s, AppSettings::default());
        let copy = fs::read_to_string(tmp.path().join("settings.json.broken")).unwrap();
        assert_eq!(copy, bad);
        // The original stays until the next save overwrites it.
        assert!(tmp.path().join(SETTINGS_FILE).exists());
    }

    #[test]
    fn no_dir_never_writes() {
        let store = Store::new(None);
        store.save_settings(&AppSettings::default()).unwrap();
        store.write_json("anything.json", &42).unwrap();
        assert_eq!(store.load_settings(), AppSettings::default());
        assert_eq!(store.read_json::<u32>("anything.json"), 0);
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let tmp = TempDir::new().unwrap();
        let store = store(&tmp);
        store.save_settings(&AppSettings::default()).unwrap();
        let changed = AppSettings {
            keep_history: false,
            ..AppSettings::default()
        };
        store.save_settings(&changed).unwrap();
        assert_eq!(names(tmp.path()), vec![SETTINGS_FILE.to_owned()]);
        assert_eq!(store.load_settings(), changed);
    }

    fn settings_with(location: DefaultLocation) -> AppSettings {
        AppSettings {
            default_output: DefaultOutput {
                location,
                subfolder_name: "done".into(),
                custom_folder: "/srv/out".into(),
            },
            ..AppSettings::default()
        }
    }

    fn default_rule() -> OutputRule {
        OutputRule {
            location: OutputLocation::Default,
            ..OutputRule::default()
        }
    }

    #[test]
    fn with_default_output_same_folder() {
        let out = settings_with(DefaultLocation::SameFolder).with_default_output(&default_rule());
        assert_eq!(out.location, OutputLocation::SameFolder);
        assert!(!out.keep_dates);
    }

    #[test]
    fn with_default_output_subfolder() {
        let out = settings_with(DefaultLocation::Subfolder).with_default_output(&default_rule());
        assert_eq!(out.location, OutputLocation::Subfolder("done".into()));
    }

    #[test]
    fn with_default_output_custom_folder() {
        let out = settings_with(DefaultLocation::CustomFolder).with_default_output(&default_rule());
        assert_eq!(
            out.location,
            OutputLocation::CustomFolder {
                path: PathBuf::from("/srv/out"),
                keep_folders_from: None,
            }
        );
    }

    #[test]
    fn with_default_output_leaves_explicit_locations_alone() {
        let rule = OutputRule {
            location: OutputLocation::Subfolder("mine".into()),
            pattern: "{name}-x".into(),
            ..OutputRule::default()
        };
        let out = settings_with(DefaultLocation::CustomFolder).with_default_output(&rule);
        assert_eq!(out, rule);
    }

    #[test]
    fn with_default_output_ors_keep_dates() {
        let mut settings = settings_with(DefaultLocation::SameFolder);
        settings.keep_file_dates = true;
        let out = settings.with_default_output(&OutputRule::default());
        assert!(out.keep_dates);

        settings.keep_file_dates = false;
        let rule = OutputRule {
            keep_dates: true,
            ..OutputRule::default()
        };
        assert!(settings.with_default_output(&rule).keep_dates);
    }

    // ---- The v2 fields (PX-04) ----

    fn load(json: &str) -> AppSettings {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join(SETTINGS_FILE), json).unwrap();
        let s = store(&tmp).load_settings();
        assert!(
            !tmp.path().join("settings.json.broken").exists(),
            "the file was declared broken: {json}"
        );
        s
    }

    #[test]
    fn defaults_are_pinned() {
        let s = AppSettings::default();
        assert_eq!(SETTINGS_SCHEMA_VERSION, 2);
        assert_eq!(s.language, "auto");
        assert!(s.start_with_system);
        assert!(s.context_menu_enabled);
        // Offline unless asked: the one place we disagree with Filee on purpose.
        assert!(!s.check_for_updates);
        assert!(!s.paused);

        assert_eq!(s.theme.mode, ThemeMode::System);
        assert_eq!(s.theme.accent, "#7F77DD");
        assert_eq!(s.theme.corner_radius, 16.0);
        assert_eq!(s.theme.reduce_motion, ReduceMotion::Auto);

        assert_eq!(s.donut.outer_radius, 150.0);
        assert_eq!(s.donut.hole_ratio, 0.42);
        assert_eq!(s.donut.opacity, 0.96);

        assert_eq!(s.triggers.len(), 2);
        let drag = &s.triggers[0];
        assert_eq!(drag.kind, TriggerKind::Drag);
        assert_eq!(drag.button, TriggerButton::Left);
        if cfg!(target_os = "macos") {
            assert!(drag.alt && !drag.ctrl);
        } else {
            assert!(drag.ctrl && !drag.alt);
        }
        assert!(!drag.shift && !drag.meta);
        assert_eq!(drag.key, None);
        let chord = &s.triggers[1];
        assert_eq!(chord.kind, TriggerKind::KeyChord);
        assert!(chord.ctrl && chord.alt && !chord.shift && !chord.meta);
        assert_eq!(chord.button, TriggerButton::None);
        assert_eq!(chord.key.as_deref(), Some("Space"));
        for t in &s.triggers {
            assert_eq!(t.hold_ms, 450);
            assert_eq!(t.drag_distance, 0);
            assert_eq!(t.scope, TriggerScope::FileManagers);
            assert!(t.enabled);
        }

        assert!(s.excluded_processes.is_empty());
        assert!(s.engine_priority.is_empty());
        assert_eq!(
            s.libre_office_for,
            LibreOfficeFor {
                slides: true,
                hwp: false,
                word: false,
                sheets: false,
            }
        );
        assert!(!s.first_run_completed);
        assert_eq!(s.successful_conversions, 0);
        assert!(!s.feedback_prompt_shown);
        assert!(s.engine_own_copies.is_empty());
        assert_eq!(s.notified_update_version, None);
        assert!(s.notified_engine_versions.is_empty());
    }

    #[test]
    fn v1_file_loads_with_defaults_for_the_new_fields() {
        // Exactly what v0.1 wrote.
        let s = load(
            r#"{
  "schemaVersion": 1,
  "keepHistory": false,
  "keepFileDates": true,
  "defaultOutput": {
    "location": "Subfolder",
    "subfolderName": "done",
    "customFolder": ""
  }
}
"#,
        );
        let expected = AppSettings {
            keep_history: false,
            keep_file_dates: true,
            default_output: DefaultOutput {
                location: DefaultLocation::Subfolder,
                subfolder_name: "done".into(),
                custom_folder: String::new(),
            },
            ..AppSettings::default()
        };
        assert_eq!(s, expected);
        assert_eq!(s.schema_version, SETTINGS_SCHEMA_VERSION);
    }

    #[test]
    fn a_newer_schema_keeps_its_number() {
        assert_eq!(load(r#"{"schemaVersion": 9}"#).schema_version, 9);
    }

    #[test]
    fn full_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = store(&tmp);
        let settings = AppSettings {
            language: "zh-CN".into(),
            start_with_system: false,
            context_menu_enabled: false,
            check_for_updates: true,
            paused: true,
            theme: ThemeSettings {
                mode: ThemeMode::Dark,
                accent: "#F04A35".into(),
                corner_radius: 4.5,
                reduce_motion: ReduceMotion::On,
            },
            donut: DonutSettings {
                outer_radius: 200.0,
                hole_ratio: 0.3,
                opacity: 0.75,
            },
            triggers: vec![TriggerGesture {
                kind: TriggerKind::Hold,
                ctrl: false,
                shift: true,
                meta: true,
                button: TriggerButton::Middle,
                key: Some("F2".into()),
                drag_distance: 12,
                hold_ms: 800,
                scope: TriggerScope::Everywhere,
                enabled: false,
                ..TriggerGesture::default()
            }],
            excluded_processes: vec!["gimp".into()],
            engine_priority: vec!["ffmpeg".into(), "image".into()],
            libre_office_for: LibreOfficeFor {
                slides: false,
                hwp: true,
                word: true,
                sheets: true,
            },
            first_run_completed: true,
            successful_conversions: 7,
            feedback_prompt_shown: true,
            engine_own_copies: [("ffmpeg".to_owned(), PathBuf::from("/opt/ffmpeg/bin/ffmpeg"))]
                .into(),
            notified_update_version: Some("0.3.0".into()),
            notified_engine_versions: [("ffmpeg".to_owned(), "7.1".to_owned())].into(),
            ..AppSettings::default()
        };
        store.save_settings(&settings).unwrap();
        assert_eq!(store.load_settings(), settings);

        let text = fs::read_to_string(tmp.path().join(SETTINGS_FILE)).unwrap();
        for key in [
            "\"startWithSystem\"",
            "\"contextMenuEnabled\"",
            "\"checkForUpdates\"",
            "\"cornerRadius\"",
            "\"reduceMotion\": \"On\"",
            "\"outerRadius\"",
            "\"holeRatio\"",
            "\"dragDistance\": 12",
            "\"holdMs\": 800",
            "\"scope\": \"Everywhere\"",
            "\"excludedProcesses\"",
            "\"enginePriority\"",
            "\"libreOfficeFor\"",
            "\"firstRunCompleted\"",
            "\"successfulConversions\": 7",
            "\"feedbackPromptShown\"",
            "\"engineOwnCopies\"",
            "\"notifiedUpdateVersion\"",
            "\"notifiedEngineVersions\"",
        ] {
            assert!(text.contains(key), "{key} missing from {text}");
        }
    }

    #[test]
    fn defaults_round_trip_and_skip_nulls() {
        let tmp = TempDir::new().unwrap();
        let store = store(&tmp);
        store.save_settings(&AppSettings::default()).unwrap();
        assert_eq!(store.load_settings(), AppSettings::default());
        let text = fs::read_to_string(tmp.path().join(SETTINGS_FILE)).unwrap();
        assert!(!text.contains("null"), "{text}");
        assert!(!text.contains("notifiedUpdateVersion"), "{text}");
    }

    #[test]
    fn numbers_are_clamped_on_load() {
        let s = load(
            r#"{
              "theme": {"cornerRadius": 99},
              "donut": {"outerRadius": 10, "holeRatio": 0.9, "opacity": 0.1},
              "triggers": [
                {"dragDistance": 500, "holdMs": 10},
                {"dragDistance": -3, "holdMs": 99999},
                {"holdMs": 450.6}
              ],
              "successfulConversions": -5
            }"#,
        );
        assert_eq!(s.theme.corner_radius, 28.0);
        assert_eq!(s.donut.outer_radius, 110.0);
        assert_eq!(s.donut.hole_ratio, 0.65);
        assert_eq!(s.donut.opacity, 0.5);
        assert_eq!(s.triggers[0].drag_distance, 200);
        assert_eq!(s.triggers[0].hold_ms, 150);
        assert_eq!(s.triggers[1].drag_distance, 0);
        assert_eq!(s.triggers[1].hold_ms, 3000);
        assert_eq!(s.triggers[2].hold_ms, 451);
        assert_eq!(s.successful_conversions, 0);

        let low = load(
            r#"{"theme": {"cornerRadius": -1}, "donut": {"outerRadius": 999, "holeRatio": 0.1, "opacity": 2}}"#,
        );
        assert_eq!(low.theme.corner_radius, 0.0);
        assert_eq!(low.donut.outer_radius, 220.0);
        assert_eq!(low.donut.hole_ratio, 0.25);
        assert_eq!(low.donut.opacity, 1.0);
    }

    #[test]
    fn odd_strings_fall_back_to_defaults() {
        let s = load(
            r##"{
              "language": "klingon",
              "theme": {"mode": "Neon", "accent": "tomato", "reduceMotion": "maybe"},
              "triggers": [{"kind": "Wiggle", "button": "Toe", "scope": "Moon", "key": "  "}],
              "excludedProcesses": ["  ", " gimp "]
            }"##,
        );
        assert_eq!(s.language, "auto");
        assert_eq!(s.theme.mode, ThemeMode::System);
        assert_eq!(s.theme.accent, DEFAULT_ACCENT);
        assert_eq!(s.theme.reduce_motion, ReduceMotion::Auto);
        assert_eq!(s.triggers, vec![TriggerGesture::default()]);
        assert_eq!(s.excluded_processes, vec!["gimp".to_owned()]);

        // Case does not matter, and the accent comes out upper case.
        let s = load(r##"{"language": "ZH-cn", "theme": {"mode": "dark", "accent": "#f04a35"}}"##);
        assert_eq!(s.language, "zh-CN");
        assert_eq!(s.theme.mode, ThemeMode::Dark);
        assert_eq!(s.theme.accent, "#F04A35");
        for bad in ["#F04A3", "#F04A35FF", "#GGGGGG", "F04A35A"] {
            assert!(!is_hex_colour(bad), "{bad}");
        }
    }

    #[test]
    fn an_empty_trigger_list_stays_empty() {
        // The user removed every gesture; that's a choice, not a missing field.
        assert!(load(r#"{"triggers": []}"#).triggers.is_empty());
    }

    #[test]
    fn filee_shaped_settings_deserialise() {
        // Shaped like a Filee 1.11 settings.json (schema 17, Windows), built
        // from AppSettings.cs and its JSON context.
        let s = load(
            r##"{
  "schemaVersion": 17,
  "language": "ko",
  "startWithSystem": false,
  "contextMenuEnabled": true,
  "checkForUpdates": true,
  "keepHistory": true,
  "notifiedUpdateVersion": "1.11.0",
  "notifiedEngineVersions": { "ffmpeg": "7.1.1" },
  "paused": false,
  "theme": { "mode": "Light", "accent": "#7F77DD", "cornerRadius": 20, "reduceMotion": true },
  "donut": { "outerRadius": 180, "holeRatio": 0.5, "opacity": 0.9 },
  "triggers": [
    { "id": "a1b2c3d4", "enabled": true, "kind": "Drag", "modifiers": "Ctrl", "button": "Left",
      "dragThreshold": 0, "holdMilliseconds": 450, "scope": "FileManager" },
    { "id": "e5f6a7b8", "enabled": true, "kind": "KeyChord", "modifiers": "Ctrl, Alt", "button": "None",
      "key": "Space", "dragThreshold": 0, "holdMilliseconds": 450, "scope": "FileManager" },
    { "id": "c9d0e1f2", "enabled": false, "kind": "Hold", "modifiers": 12, "button": "Right",
      "dragThreshold": 16, "holdMilliseconds": 700, "scope": "Anywhere" }
  ],
  "excludedProcesses": [ "photoshop" ],
  "enginePriority": [ "magick", "pdfium", "ffmpeg" ],
  "firstRunCompleted": true,
  "successfulConversions": 3,
  "feedbackPromptShown": false,
  "watchFolders": [],
  "defaultOutput": { "location": "CustomFolder", "subfolderName": "converted", "customFolder": "D:\\out" },
  "engineOwnCopies": { "calibre": "C:\\Program Files\\Calibre2\\ebook-convert.exe" },
  "libreOfficeForSlides": false,
  "libreOfficeForHwp": true,
  "libreOfficeForDocuments": true,
  "libreOfficeForSheets": false,
  "keepFileDates": false,
}"##
            // Filee allows trailing commas; serde_json doesn't. Fair enough.
            .replace(",\n}", "\n}")
            .as_str(),
        );
        assert_eq!(s.schema_version, 17);
        assert_eq!(s.language, "ko");
        assert!(!s.start_with_system);
        assert!(s.check_for_updates);
        assert_eq!(s.notified_update_version.as_deref(), Some("1.11.0"));
        assert_eq!(s.notified_engine_versions["ffmpeg"], "7.1.1");
        assert_eq!(s.theme.mode, ThemeMode::Light);
        assert_eq!(s.theme.corner_radius, 20.0);
        assert_eq!(s.theme.reduce_motion, ReduceMotion::On);
        assert_eq!(
            s.donut,
            DonutSettings {
                outer_radius: 180.0,
                hole_ratio: 0.5,
                opacity: 0.9,
            }
        );

        assert_eq!(s.triggers.len(), 3);
        let [drag, chord, hold] = [&s.triggers[0], &s.triggers[1], &s.triggers[2]];
        assert!(drag.ctrl && !drag.alt);
        assert_eq!(drag.scope, TriggerScope::FileManagers);
        assert_eq!(chord.kind, TriggerKind::KeyChord);
        assert!(chord.ctrl && chord.alt);
        assert_eq!(chord.button, TriggerButton::None);
        assert_eq!(chord.key.as_deref(), Some("Space"));
        assert_eq!(hold.kind, TriggerKind::Hold);
        assert!(!hold.ctrl && !hold.alt && hold.shift && hold.meta);
        assert_eq!(hold.button, TriggerButton::Right);
        assert_eq!(hold.drag_distance, 16);
        assert_eq!(hold.hold_ms, 700);
        assert_eq!(hold.scope, TriggerScope::Everywhere);
        assert!(!hold.enabled);

        assert_eq!(s.excluded_processes, vec!["photoshop".to_owned()]);
        assert_eq!(s.engine_priority, vec!["magick", "pdfium", "ffmpeg"]);
        assert!(s.first_run_completed);
        assert_eq!(s.successful_conversions, 3);
        assert_eq!(s.default_output.location, DefaultLocation::CustomFolder);
        assert_eq!(
            s.engine_own_copies["calibre"],
            PathBuf::from(r"C:\Program Files\Calibre2\ebook-convert.exe")
        );
        assert_eq!(
            s.libre_office_for,
            LibreOfficeFor {
                slides: false,
                hwp: true,
                word: true,
                sheets: false,
            }
        );
    }

    #[test]
    fn our_bools_beat_filee_modifier_flags() {
        let s = load(r#"{"triggers": [{"modifiers": "Ctrl, Shift", "ctrl": false}]}"#);
        let t = &s.triggers[0];
        assert!(!t.ctrl && t.shift && !t.alt && !t.meta);
    }

    #[test]
    fn nested_libre_office_beats_nothing_and_flat_keys_override_it() {
        let s = load(r#"{"libreOfficeFor": {"sheets": true}}"#);
        assert!(s.libre_office_for.slides && s.libre_office_for.sheets);
        let s = load(r#"{"libreOfficeFor": {"documents": true}, "libreOfficeForSlides": false}"#);
        assert!(s.libre_office_for.word);
        assert!(!s.libre_office_for.slides);
    }

    #[test]
    fn reduce_motion_reads_every_spelling() {
        for (json, want) in [
            (r#"{"theme": {"reduceMotion": null}}"#, ReduceMotion::Auto),
            (r#"{"theme": {"reduceMotion": false}}"#, ReduceMotion::Off),
            (r#"{"theme": {"reduceMotion": "off"}}"#, ReduceMotion::Off),
            (r#"{"theme": {"reduceMotion": "Auto"}}"#, ReduceMotion::Auto),
            (r#"{"theme": {}}"#, ReduceMotion::Auto),
        ] {
            assert_eq!(load(json).theme.reduce_motion, want, "{json}");
        }
    }
}
