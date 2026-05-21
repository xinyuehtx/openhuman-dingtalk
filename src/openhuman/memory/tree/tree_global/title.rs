//! Chinese display titles for global-tree summary files.
//!
//! Global summaries are organised on a time axis (L0=day, L1=week, L2=month,
//! L3+=year). The Obsidian-facing filename and `aliases:` value should reflect
//! that semantics in Chinese so a user browsing
//! `wiki/summaries/global-…/L2/…` immediately sees `2026年5月.md` instead of
//! a date range.
//!
//! Format by level:
//!
//! | Level | Semantics | Example         |
//! |-------|-----------|-----------------|
//! | 0     | Day       | `2026年5月21日` |
//! | 1     | Week      | `2026年5月第3周`|
//! | 2     | Month     | `2026年5月`     |
//! | ≥3    | Year      | `2026年`        |
//!
//! When the start and end of the requested range straddle the natural
//! boundary for that level (e.g. an L1 weekly seal whose
//! `[start, end]` window spans two ISO weeks), the formatter still keys off
//! `start`; the actual disambiguation between adjacent weeks lives in the
//! summary id and the date-segmented directory name produced by
//! `summary_rel_path`.

use chrono::{DateTime, Datelike, Utc};

/// Return the Chinese display title for a global-tree summary at the given
/// level. `start` is the inclusive start of the summary's covered range.
///
/// `end` is only consulted as a tiebreaker when the level boundaries don't
/// line up cleanly with the requested range — today that path is unreached
/// because both `digest.rs` (L0) and `seal.rs` (L1+) pass the same date for
/// start and end after `align_range_to_level`. Kept as a parameter so future
/// callers that summarise an arbitrary span can render a `2026年5月–6月`
/// range without changing the public signature.
pub fn chinese_global_title(level: u32, start: DateTime<Utc>, end: DateTime<Utc>) -> String {
    match level {
        0 => format_day(start),
        1 => format_week(start),
        2 => format_month(start),
        _ => format_year(start),
    }
    .pipe(|formatted| {
        // Cross-period sanity check: if `end`'s natural unit at this level
        // differs from `start`'s, append a range hint so the file is
        // distinguishable from neighbours that share the start label.
        // Today this never fires for digest/seal callers; it's a guard for
        // future ad-hoc recap callers that build summaries over a window
        // straddling boundaries (e.g. an end-of-quarter retrospective).
        let end_formatted = match level {
            0 => format_day(end),
            1 => format_week(end),
            2 => format_month(end),
            _ => format_year(end),
        };
        if end_formatted == formatted {
            formatted
        } else {
            format!("{formatted}–{end_formatted}")
        }
    })
}

fn format_day(ts: DateTime<Utc>) -> String {
    format!("{}年{}月{}日", ts.year(), ts.month(), ts.day())
}

/// "yyyy年m月第N周" — N counted as ceil(day_of_month / 7) so the 1st falls
/// in 第1周 and the 8th in 第2周. ISO week numbers cross month boundaries
/// and would render confusingly for users browsing `wiki/summaries/global-…/L1/`.
fn format_week(ts: DateTime<Utc>) -> String {
    let week_in_month = ((ts.day() - 1) / 7) + 1;
    format!("{}年{}月第{}周", ts.year(), ts.month(), week_in_month)
}

fn format_month(ts: DateTime<Utc>) -> String {
    format!("{}年{}月", ts.year(), ts.month())
}

fn format_year(ts: DateTime<Utc>) -> String {
    format!("{}年", ts.year())
}

/// Local extension trait to keep the match arms compact above. Not exported.
trait Pipe: Sized {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R,
    {
        f(self)
    }
}

impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
    }

    #[test]
    fn level_0_renders_day() {
        let t = ts(2026, 5, 21);
        assert_eq!(chinese_global_title(0, t, t), "2026年5月21日");
    }

    #[test]
    fn level_0_drops_leading_zeros() {
        let t = ts(2026, 1, 5);
        assert_eq!(chinese_global_title(0, t, t), "2026年1月5日");
    }

    #[test]
    fn level_1_renders_week_in_month() {
        // Day 1 → week 1
        assert_eq!(
            chinese_global_title(1, ts(2026, 5, 1), ts(2026, 5, 1)),
            "2026年5月第1周"
        );
        // Day 7 → week 1 (boundary)
        assert_eq!(
            chinese_global_title(1, ts(2026, 5, 7), ts(2026, 5, 7)),
            "2026年5月第1周"
        );
        // Day 8 → week 2 (just past boundary)
        assert_eq!(
            chinese_global_title(1, ts(2026, 5, 8), ts(2026, 5, 8)),
            "2026年5月第2周"
        );
        // Day 21 → week 3
        assert_eq!(
            chinese_global_title(1, ts(2026, 5, 21), ts(2026, 5, 21)),
            "2026年5月第3周"
        );
        // Day 31 → week 5 (caps at fifth)
        assert_eq!(
            chinese_global_title(1, ts(2026, 5, 31), ts(2026, 5, 31)),
            "2026年5月第5周"
        );
    }

    #[test]
    fn level_2_renders_month_only() {
        let t = ts(2026, 5, 15);
        assert_eq!(chinese_global_title(2, t, t), "2026年5月");
    }

    #[test]
    fn level_3_renders_year_only() {
        let t = ts(2026, 8, 1);
        assert_eq!(chinese_global_title(3, t, t), "2026年");
    }

    #[test]
    fn level_above_3_still_renders_year() {
        // Defensive: future tree depths fall through to year, not panic.
        let t = ts(2026, 8, 1);
        assert_eq!(chinese_global_title(7, t, t), "2026年");
    }

    #[test]
    fn cross_period_emits_range() {
        // Cross-month L2 fixture exercises the end-formatter divergence
        // branch — today unreached by digest/seal but kept correct so a
        // future ad-hoc recap caller gets a sensible label.
        let start = ts(2026, 4, 30);
        let end = ts(2026, 5, 1);
        assert_eq!(chinese_global_title(2, start, end), "2026年4月–2026年5月");
    }

    #[test]
    fn same_period_returns_single_label() {
        // Regression: same-day start/end at L0 must not append en dash.
        let t = ts(2026, 5, 21);
        let formatted = chinese_global_title(0, t, t);
        assert!(
            !formatted.contains('\u{2013}'),
            "same-day L0 must not include en dash; got {formatted}"
        );
    }
}
