//! Page-range parsing (`1-3,5,8-`) for PDF splitting and extraction.
//! Ported from Filee's `src/Filee.Core/Conversion/PageRange.cs`.
//!
//! The parser is forgiving on purpose: a typo in the page box should never
//! fail a conversion, it just quietly falls back to "every page".

/// Parses a 1-based page range expression into sorted, distinct, 0-based
/// page indices.
///
/// Supported entries, separated by commas: `3`, `2-5`, `7-` (to the last
/// page) and `-4` (from the first page). Invalid entries are ignored,
/// reversed ranges are swapped and pages outside `1..=page_count` are
/// dropped. A blank expression, or one with no valid page, selects every
/// page. A `page_count` of 0 always yields an empty list.
pub fn parse(expr: &str, page_count: usize) -> Vec<usize> {
    if page_count == 0 {
        return Vec::new();
    }
    let all = || (0..page_count).collect::<Vec<_>>();
    if expr.trim().is_empty() {
        return all();
    }

    // A bitmap beats a BTreeSet here: page counts are small and it hands us
    // sorted + distinct for free, which is exactly the contract.
    let mut picked = vec![false; page_count];
    let last = page_count as i64;
    for entry in expr.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((start, end)) = bounds(entry, last) else {
            continue;
        };
        let (start, end) = if start > end {
            (end, start)
        } else {
            (start, end)
        };
        for page in start.max(1)..=end.min(last) {
            picked[(page - 1) as usize] = true;
        }
    }

    let pages: Vec<usize> = picked
        .iter()
        .enumerate()
        .filter_map(|(i, &on)| on.then_some(i))
        .collect();
    if pages.is_empty() { all() } else { pages }
}

/// One entry as an unclamped 1-based `(start, end)`, or `None` when it is not
/// a number or a range. The first `-` is the separator, so `-3` means "up to
/// 3" and `1--3` is garbage, same as in Filee.
fn bounds(entry: &str, last: i64) -> Option<(i64, i64)> {
    let Some((left, right)) = entry.split_once('-') else {
        let n = number(entry)?;
        return Some((n, n));
    };
    let (left, right) = (left.trim(), right.trim());
    let start = if left.is_empty() { 1 } else { number(left)? };
    let end = if right.is_empty() {
        last
    } else {
        number(right)?
    };
    (start >= 0 && end >= 0).then_some((start, end))
}

/// Filee leans on `int.TryParse`, so anything that would overflow an `i32`
/// counts as junk rather than "a very big page".
fn number(s: &str) -> Option<i64> {
    s.parse::<i32>().ok().map(i64::from)
}

/// Groups 0-based page indices into contiguous 1-based `(first, last)` runs,
/// the shape `pdftoppm -f/-l` and Ghostscript's `-dFirstPage/-dLastPage` want.
///
/// Expects the sorted, distinct output of [`parse`]; any break in the
/// sequence simply starts a new run.
pub fn runs(indices: &[usize]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for &index in indices {
        let page = index + 1;
        match out.last_mut() {
            Some((_, last)) if *last + 1 == page => *last = page,
            _ => out.push((page, page)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every case from Filee's PageRangeTests (SmallParsersTests.cs).
    #[test]
    fn filee_cases() {
        let cases: &[(&str, usize, &[usize])] = &[
            ("", 5, &[0, 1, 2, 3, 4]),
            ("2", 5, &[1]),
            ("2-4", 5, &[1, 2, 3]),
            ("4-", 5, &[3, 4]),
            ("-2", 5, &[0, 1]),
            ("5,1, 3", 5, &[0, 2, 4]),
            ("4-2", 5, &[1, 2, 3]),
            ("9-12", 5, &[0, 1, 2, 3, 4]),
            ("x, 2", 5, &[1]),
        ];
        for &(expr, count, expected) in cases {
            assert_eq!(parse(expr, count), expected, "parse({expr:?}, {count})");
        }
    }

    #[test]
    fn zero_pages_is_always_empty() {
        assert!(parse("", 0).is_empty());
        assert!(parse("1-3", 0).is_empty());
    }

    #[test]
    fn whitespace_only_means_every_page() {
        assert_eq!(parse("  \t ", 3), [0, 1, 2]);
    }

    #[test]
    fn overlaps_and_duplicates_collapse() {
        assert_eq!(parse("1-3, 2-4, 3, 3", 5), [0, 1, 2, 3]);
    }

    #[test]
    fn ranges_are_clamped_to_the_document() {
        assert_eq!(parse("0-2", 5), [0, 1]);
        assert_eq!(parse("4-99", 5), [3, 4]);
        assert_eq!(parse("0", 5), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn junk_falls_back_to_every_page() {
        assert_eq!(parse("abc", 3), [0, 1, 2]);
        assert_eq!(parse(",,,", 3), [0, 1, 2]);
        assert_eq!(parse("1--3", 3), [0, 1, 2]);
        assert_eq!(parse("a-2", 3), [0, 1, 2]);
        assert_eq!(parse("99999999999", 3), [0, 1, 2]);
    }

    #[test]
    fn lone_dash_and_spaced_ranges() {
        assert_eq!(parse("-", 3), [0, 1, 2]);
        assert_eq!(parse(" 2 - 3 ", 4), [1, 2]);
        assert_eq!(parse("+2", 4), [1]);
    }

    #[test]
    fn runs_group_contiguous_pages() {
        assert_eq!(runs(&[]), []);
        assert_eq!(runs(&[0]), [(1, 1)]);
        assert_eq!(runs(&[0, 1, 2]), [(1, 3)]);
        assert_eq!(runs(&[0, 2, 4]), [(1, 1), (3, 3), (5, 5)]);
        assert_eq!(runs(&[1, 2, 3, 6, 7, 9]), [(2, 4), (7, 8), (10, 10)]);
    }

    #[test]
    fn runs_round_trip_a_parse() {
        assert_eq!(runs(&parse("1-3,5,8-", 10)), [(1, 3), (5, 5), (8, 10)]);
    }
}
