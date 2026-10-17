//! Name rules: small edits (replace, prefix, strip copy numbers, ...) applied
//! to an output's file name after the pattern is expanded and before it is
//! sanitised. Ported from Filee's `RenameStep` in
//! `src/Filee.Core/Presets/OutputRule.cs` and the `Rename` helpers of
//! `OutputPathResolver.cs`.
//!
//! The JSON matches Filee's: `{"kind": "Replace", "find": "IMG_", "replace":
//! "Photo_"}`, so a Filee preset file with name rules loads as is.

use std::sync::LazyLock;

use regex::{NoExpand, Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

/// How big a compiled user expression may get. The regex crate never
/// backtracks, so there is no runaway match to time out like Filee's 250 ms
/// guard; the only way to hurt us is a monster pattern, and this caps it.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// One change to the name of a converted file, e.g. "replace IMG_ with Photo_"
/// or "spaces to underscores". Steps run in order; see [`apply_all`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawStep", into = "RawStep")]
pub enum RenameStep {
    /// Replaces every `find` with `with`. Case is ignored unless `match_case`.
    Replace {
        find: String,
        with: String,
        match_case: bool,
    },
    /// Removes every `text`, ignoring case.
    Remove { text: String },
    /// Puts `text` before the name.
    Prefix { text: String },
    /// Puts `text` after the name.
    Suffix { text: String },
    /// `my trip photo` becomes `my_trip_photo`.
    SpacesToUnderscores,
    /// `report (2)` becomes `report`.
    RemoveCopyNumber,
    /// `song [remix] (live)` becomes `song`.
    RemoveBrackets,
    /// `01 - intro` becomes `intro`.
    RemoveLeadingNumber,
    /// `invoice#12@acme!` becomes `invoice12acme`.
    RemoveSymbols,
    /// `Report_FINAL` becomes `report_final`.
    LowerCase,
    /// `report` becomes `REPORT`.
    UpperCase,
    /// Replaces the regular expression `pattern` with `replacement`, where `$1`
    /// inserts a group. An invalid pattern leaves the name unchanged.
    Regex {
        pattern: String,
        replacement: String,
    },
}

impl RenameStep {
    /// True for a step that changes nothing because its text is still missing
    /// (a step the user is halfway through typing).
    pub fn is_blank(&self) -> bool {
        match self {
            Self::Replace { find, .. } => find.is_empty(),
            Self::Remove { text } => text.is_empty(),
            Self::Regex { pattern, .. } => pattern.is_empty(),
            Self::Prefix { text } | Self::Suffix { text } => text.is_empty(),
            _ => false,
        }
    }

    /// The name after this step.
    pub fn apply(&self, name: &str) -> String {
        if self.is_blank() {
            return name.to_owned();
        }
        match self {
            Self::Replace {
                find,
                with,
                match_case: true,
            } => name.replace(find.as_str(), with),
            Self::Replace { find, with, .. } => replace_ignoring_case(name, find, with),
            Self::Remove { text } => replace_ignoring_case(name, text, ""),
            Self::Prefix { text } => format!("{text}{name}"),
            Self::Suffix { text } => format!("{name}{text}"),
            Self::SpacesToUnderscores => fixed(&SPACES, name, "_"),
            Self::RemoveCopyNumber => fixed(&COPY_NUMBER, name, ""),
            Self::RemoveBrackets => fixed(&BRACKETS, name, "").trim().to_owned(),
            Self::RemoveLeadingNumber => fixed(&LEADING_NUMBER, name, ""),
            Self::RemoveSymbols => fixed(&SYMBOLS, name, ""),
            Self::LowerCase => name.to_lowercase(),
            Self::UpperCase => name.to_uppercase(),
            Self::Regex {
                pattern,
                replacement,
            } => regex_replace(name, pattern, replacement),
        }
    }
}

/// Applies `steps` to `name` in order.
pub fn apply_all(name: &str, steps: &[RenameStep]) -> String {
    steps
        .iter()
        .fold(name.to_owned(), |current, step| step.apply(&current))
}

/// Replaces the regular expression `pattern` in `name` with `replacement`
/// (Filee/.NET syntax: `$1`, `${name}`, `$&`, `$$`). A pattern that does not
/// compile leaves the name as it is.
pub fn regex_replace(name: &str, pattern: &str, replacement: &str) -> String {
    if pattern.is_empty() {
        return name.to_owned();
    }
    match compile(pattern) {
        Ok(re) => re
            .replace_all(name, dotnet_replacement(replacement).as_str())
            .into_owned(),
        Err(_) => name.to_owned(),
    }
}

/// Why `pattern` is not a usable regular expression, or `None` when it is (or
/// is empty). For the settings UI, which wants to say so before the user saves.
pub fn regex_error(pattern: &str) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }
    compile(pattern).err().map(|e| e.to_string())
}

fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
}

/// Turns a .NET replacement string into one the regex crate reads the same way.
///
/// The two mostly agree, except for one trap: the regex crate reads `$1_$2` as
/// the group named `1_` (which doesn't exist, so it vanishes), while .NET reads
/// group 1 followed by `_`. Filee users write exactly that, so numbers get
/// braces. `$&` is the whole match; any other `$` that isn't a reference is a
/// literal dollar, like in .NET.
fn dotnet_replacement(replacement: &str) -> String {
    let mut out = String::with_capacity(replacement.len() + 8);
    let mut chars = replacement.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().map(|&(_, n)| n) {
            Some('$') => {
                chars.next();
                out.push_str("$$");
            }
            Some('&') => {
                chars.next();
                out.push_str("${0}");
            }
            Some(d) if d.is_ascii_digit() => {
                out.push_str("${");
                while let Some(&(_, d)) = chars.peek().filter(|(_, d)| d.is_ascii_digit()) {
                    out.push(d);
                    chars.next();
                }
                out.push('}');
            }
            Some('{') => match replacement[i..].find('}') {
                Some(close) => {
                    out.push_str(&replacement[i..i + close + 1]);
                    // Skip past the closing brace; the '$' itself is already consumed.
                    while chars.next_if(|&(j, _)| j <= i + close).is_some() {}
                }
                None => out.push_str("$$"),
            },
            _ => out.push_str("$$"),
        }
    }
    out
}

/// .NET's `Replace(find, with, OrdinalIgnoreCase)`, via an escaped pattern so
/// we get Unicode case folding without hand-rolling it.
fn replace_ignoring_case(name: &str, find: &str, with: &str) -> String {
    match RegexBuilder::new(&regex::escape(find))
        .case_insensitive(true)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
    {
        Ok(re) => re.replace_all(name, NoExpand(with)).into_owned(),
        // Only a find text of absurd length gets here; a no-op beats a panic.
        Err(_) => name.to_owned(),
    }
}

fn fixed(re: &Regex, name: &str, with: &str) -> String {
    re.replace_all(name, NoExpand(with)).into_owned()
}

// Filee's built-in expressions, verbatim. Rust's \s and \d are Unicode-aware by
// default, same as .NET's, so "사진 (2)" loses its copy number just fine.
#[allow(clippy::expect_used, reason = "literal pattern, covered by the tests")]
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid"));
#[allow(clippy::expect_used, reason = "literal pattern, covered by the tests")]
static COPY_NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d+\)$").expect("valid"));
#[allow(clippy::expect_used, reason = "literal pattern, covered by the tests")]
static BRACKETS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*(\([^)]*\)|\[[^\]]*\]|\{[^}]*\})").expect("valid"));
#[allow(clippy::expect_used, reason = "literal pattern, covered by the tests")]
static LEADING_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+[\s._-]*").expect("valid"));
#[allow(clippy::expect_used, reason = "literal pattern, covered by the tests")]
static SYMBOLS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}\s._-]").expect("valid"));

/// Filee's on-disk shape: a kind plus two strings whose meaning depends on it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RawStep {
    kind: String,
    find: String,
    replace: String,
    /// MorphRing's addition; Filee always ignored case. Left out when false so
    /// the file still reads like Filee's.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    match_case: bool,
}

impl From<RawStep> for RenameStep {
    fn from(raw: RawStep) -> Self {
        let RawStep {
            kind,
            find,
            replace,
            match_case,
        } = raw;
        // Kind names match case-insensitively, like Filee's string enum reader.
        match kind.to_ascii_lowercase().as_str() {
            "remove" => Self::Remove { text: find },
            "prefix" => Self::Prefix { text: replace },
            "suffix" => Self::Suffix { text: replace },
            "spacestounderscores" => Self::SpacesToUnderscores,
            "removecopynumber" => Self::RemoveCopyNumber,
            "removebrackets" => Self::RemoveBrackets,
            "removeleadingnumber" => Self::RemoveLeadingNumber,
            "removesymbols" => Self::RemoveSymbols,
            "lowercase" => Self::LowerCase,
            "uppercase" => Self::UpperCase,
            "regex" => Self::Regex {
                pattern: find,
                replacement: replace,
            },
            // "replace" (Filee's default kind), a missing kind, or one from a
            // newer version. An unknown kind keeps its texts but gets an empty
            // find, so it is blank and does nothing rather than guessing.
            "replace" | "" => Self::Replace {
                find,
                with: replace,
                match_case,
            },
            _ => Self::Replace {
                find: String::new(),
                with: String::new(),
                match_case: false,
            },
        }
    }
}

impl From<RenameStep> for RawStep {
    fn from(step: RenameStep) -> Self {
        let raw = |kind: &str, find: String, replace: String| RawStep {
            kind: kind.to_owned(),
            find,
            replace,
            match_case: false,
        };
        match step {
            RenameStep::Replace {
                find,
                with,
                match_case,
            } => RawStep {
                match_case,
                ..raw("Replace", find, with)
            },
            RenameStep::Remove { text } => raw("Remove", text, String::new()),
            RenameStep::Prefix { text } => raw("Prefix", String::new(), text),
            RenameStep::Suffix { text } => raw("Suffix", String::new(), text),
            RenameStep::SpacesToUnderscores => raw("SpacesToUnderscores", "".into(), "".into()),
            RenameStep::RemoveCopyNumber => raw("RemoveCopyNumber", "".into(), "".into()),
            RenameStep::RemoveBrackets => raw("RemoveBrackets", "".into(), "".into()),
            RenameStep::RemoveLeadingNumber => raw("RemoveLeadingNumber", "".into(), "".into()),
            RenameStep::RemoveSymbols => raw("RemoveSymbols", "".into(), "".into()),
            RenameStep::LowerCase => raw("Lowercase", "".into(), "".into()),
            RenameStep::UpperCase => raw("Uppercase", "".into(), "".into()),
            RenameStep::Regex {
                pattern,
                replacement,
            } => raw("Regex", pattern, replacement),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace(find: &str, with: &str) -> RenameStep {
        RenameStep::Replace {
            find: find.into(),
            with: with.into(),
            match_case: false,
        }
    }

    fn text(s: &str) -> String {
        s.to_owned()
    }

    fn regex(pattern: &str, replacement: &str) -> RenameStep {
        RenameStep::Regex {
            pattern: pattern.into(),
            replacement: replacement.into(),
        }
    }

    // Filee's WatchRuleOptionsTests.Each_kind_of_rule_changes_the_name_as_it_says,
    // one case per step.

    #[test]
    fn rename_replace_ignores_case() {
        assert_eq!(replace("IMG_", "Photo_").apply("IMG_0412"), "Photo_0412");
        assert_eq!(replace("img_", "Photo_").apply("IMG_0412"), "Photo_0412");
    }

    #[test]
    fn rename_replace_can_match_case() {
        let step = RenameStep::Replace {
            find: "img".into(),
            with: "pic".into(),
            match_case: true,
        };
        assert_eq!(step.apply("IMG_img"), "IMG_pic");
    }

    #[test]
    fn rename_replace_treats_dollars_and_dots_literally() {
        assert_eq!(replace(".", "$1").apply("a.b.c"), "a$1b$1c");
    }

    #[test]
    fn rename_remove() {
        let step = RenameStep::Remove {
            text: text("_copy"),
        };
        assert_eq!(step.apply("report_COPY"), "report");
    }

    #[test]
    fn rename_prefix_and_suffix() {
        let prefix = RenameStep::Prefix {
            text: text("2026_"),
        };
        let suffix = RenameStep::Suffix {
            text: text("_final"),
        };
        assert_eq!(prefix.apply("report"), "2026_report");
        assert_eq!(suffix.apply("report"), "report_final");
    }

    #[test]
    fn rename_spaces_to_underscores() {
        assert_eq!(
            RenameStep::SpacesToUnderscores.apply("trip to  busan"),
            "trip_to_busan"
        );
    }

    #[test]
    fn rename_remove_copy_number() {
        let step = RenameStep::RemoveCopyNumber;
        assert_eq!(step.apply("report (2)"), "report");
        assert_eq!(step.apply("IMG_0001 (2)"), "IMG_0001");
        // Only at the end: a number in the middle is part of the name.
        assert_eq!(step.apply("a (2) b"), "a (2) b");
    }

    #[test]
    fn rename_remove_brackets() {
        assert_eq!(
            RenameStep::RemoveBrackets.apply("song [remix] (live)"),
            "song"
        );
        assert_eq!(RenameStep::RemoveBrackets.apply("a {x} b"), "a b");
    }

    #[test]
    fn rename_remove_leading_number() {
        assert_eq!(RenameStep::RemoveLeadingNumber.apply("01 - intro"), "intro");
        assert_eq!(
            RenameStep::RemoveLeadingNumber.apply("intro 01"),
            "intro 01"
        );
    }

    #[test]
    fn rename_remove_symbols() {
        assert_eq!(
            RenameStep::RemoveSymbols.apply("invoice#12@acme!"),
            "invoice12acme"
        );
        assert_eq!(RenameStep::RemoveSymbols.apply("보고서#1"), "보고서1");
        assert_eq!(RenameStep::RemoveSymbols.apply("a b.c_d-e"), "a b.c_d-e");
    }

    #[test]
    fn rename_lower_and_upper_case() {
        assert_eq!(RenameStep::LowerCase.apply("Report_FINAL"), "report_final");
        assert_eq!(RenameStep::UpperCase.apply("report"), "REPORT");
    }

    #[test]
    fn rename_regex_with_groups() {
        assert_eq!(
            regex(r"(\d{4})(\d{2})(\d{2})", "$1-$2-$3").apply("scan_20261011"),
            "scan_2026-10-11"
        );
    }

    #[test]
    fn rename_regex_reads_dotnet_replacements() {
        // The trap: the regex crate alone would read "$1_" as a group named "1_".
        assert_eq!(
            regex(r"IMG_(\d+)", "Photo_$1_x").apply("IMG_0412"),
            "Photo_0412_x"
        );
        assert_eq!(regex(r"(?<n>\d+)", "#${n}").apply("a12"), "a#12");
        assert_eq!(regex(r"\d+", "[$&]").apply("a12"), "a[12]");
        assert_eq!(regex(r"\d+", "$$").apply("a12"), "a$");
        assert_eq!(regex(r"\d+", "US$ ").apply("a12"), "aUS$ ");
        assert_eq!(regex(r"\d+", "${oops").apply("a12"), "a${oops");
    }

    #[test]
    fn rename_invalid_regex_is_a_no_op() {
        assert_eq!(regex("IMG_(", "x").apply("IMG_0412"), "IMG_0412");
        assert_eq!(regex_replace("IMG_0412", "IMG_(", "x"), "IMG_0412");
        assert!(regex_error("IMG_(").is_some());
        assert_eq!(regex_error(r"IMG_(\d+)"), None);
        assert_eq!(regex_error(""), None);
    }

    #[test]
    fn rename_oversized_regex_is_a_no_op() {
        // Big enough to blow the size limit once compiled.
        let pattern = r"\w{1000}".repeat(20);
        assert!(regex_error(&pattern).is_some());
        assert_eq!(regex(&pattern, "x").apply("name"), "name");
    }

    #[test]
    fn rename_blank_steps_change_nothing() {
        assert!(replace("", "x").is_blank());
        assert_eq!(replace("", "x").apply("report"), "report");
        assert_eq!(
            RenameStep::Prefix { text: text("") }.apply("report"),
            "report"
        );
        assert_eq!(regex("", "x").apply("report"), "report");
        assert!(!RenameStep::LowerCase.is_blank());
    }

    #[test]
    fn rename_order_matters() {
        let a = [replace("img_", "Photo "), RenameStep::SpacesToUnderscores];
        let b = [RenameStep::SpacesToUnderscores, replace("img_", "Photo ")];
        assert_eq!(apply_all("IMG_0412", &a), "Photo_0412");
        assert_eq!(apply_all("IMG_0412", &b), "Photo 0412");
        assert_eq!(apply_all("IMG_0412", &[]), "IMG_0412");
    }

    #[test]
    fn rename_steps_use_filee_json() {
        let json = r#"[
            {"kind": "Regex", "find": "IMG_(\\d+)", "replace": "Photo_$1"},
            {"kind": "Replace", "find": "a", "replace": "b"},
            {"find": "c", "replace": "d"},
            {"kind": "prefix", "replace": "x_"},
            {"kind": "Lowercase"},
            {"kind": "Uppercase"},
            {"kind": "Remove", "find": "_copy"},
            {"kind": "TeleportToMars", "find": "a", "replace": "z"}
        ]"#;
        let steps: Vec<RenameStep> = serde_json::from_str(json).unwrap();
        assert_eq!(steps[0], regex(r"IMG_(\d+)", "Photo_$1"));
        assert_eq!(steps[1], replace("a", "b"));
        assert_eq!(steps[2], replace("c", "d"));
        assert_eq!(steps[3], RenameStep::Prefix { text: text("x_") });
        assert_eq!(steps[4], RenameStep::LowerCase);
        assert_eq!(steps[5], RenameStep::UpperCase);
        assert_eq!(
            steps[6],
            RenameStep::Remove {
                text: text("_copy")
            }
        );
        assert!(steps[7].is_blank());

        let out = serde_json::to_value(&steps[..7]).unwrap();
        assert_eq!(out[0]["kind"], "Regex");
        assert_eq!(out[0]["find"], r"IMG_(\d+)");
        assert_eq!(out[0]["replace"], "Photo_$1");
        assert_eq!(out[3]["replace"], "x_");
        assert_eq!(out[4]["kind"], "Lowercase");
        assert!(out[1].get("matchCase").is_none());
        let back: Vec<RenameStep> = serde_json::from_value(out).unwrap();
        assert_eq!(back, steps[..7]);

        let cased = RenameStep::Replace {
            find: "a".into(),
            with: "b".into(),
            match_case: true,
        };
        let value = serde_json::to_value(&cased).unwrap();
        assert_eq!(value["matchCase"], true);
        assert_eq!(serde_json::from_value::<RenameStep>(value).unwrap(), cased);
    }
}
