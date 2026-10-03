//! 検索語の解析。Everything の検索構文のうち `ext:` `size:` `dm:` を読み、残りをあいまい検索に回す。
//!
//! | 書き方 | 意味 |
//! |---|---|
//! | `ext:png;jpg` | 拡張子（`;` か `,` 区切り） |
//! | `size:>10mb` `size:<=500kb` `size:1mb..10mb` `size:10mb` | サイズ（1024 単位。単位無しはバイト。単独の値は「その単位で同じ整数」） |
//! | `size:empty` `tiny` `small` `medium` `large` `huge` `gigantic` | Everything と同じ区分 |
//! | `dm:today` `yesterday` `thisweek` `lastweek` `thismonth` `lastmonth` `thisyear` `lastyear` | 更新日時（週は日曜始まり） |
//! | `dm:last3days` `dm:past2hours` | いまから遡った範囲（minute / hour / day / week / month=30日 / year=365日） |
//! | `dm:2026/09/01` `dm:>=2026-09` `dm:2026/09/01..2026/09/15` | 日付（年だけ・年月だけも可） |
//!
//! 同じ種類の条件を複数書くとすべてを満たすもの（AND）。解釈できない条件は無視して `errors` に残す。

use std::collections::HashSet;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone};

use crate::index::Entry;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    /// あいまい検索に回す残り
    pub text: String,
    pub exts: Vec<HashSet<String>>,
    /// [下限, 上限) のバイト数
    pub sizes: Vec<(u64, u64)>,
    /// [下限, 上限) の UNIX 秒
    pub dates: Vec<(i64, i64)>,
    pub errors: Vec<String>,
}

impl Query {
    pub fn parse(input: &str, now: DateTime<Local>) -> Self {
        let mut q = Query::default();
        let mut rest = Vec::new();
        for token in input.split_whitespace() {
            let lower = token.to_lowercase();
            if let Some(v) = lower.strip_prefix("ext:") {
                q.exts.push(
                    v.split([';', ','])
                        .map(|s| s.trim_start_matches('.').to_string())
                        .collect(),
                );
            } else if let Some(v) = lower.strip_prefix("size:") {
                match parse_size_cond(v) {
                    Some(r) => q.sizes.push(r),
                    None => q.errors.push(token.to_string()),
                }
            } else if let Some(v) = lower
                .strip_prefix("dm:")
                .or_else(|| lower.strip_prefix("datemodified:"))
            {
                match parse_date_cond(v, now) {
                    Some(r) => q.dates.push(r),
                    None => q.errors.push(token.to_string()),
                }
            } else {
                rest.push(token);
            }
        }
        q.text = rest.join(" ");
        q
    }

    /// あいまい検索以外の条件を満たすか
    pub fn matches_attrs(&self, e: &Entry) -> bool {
        self.exts.iter().all(|set| set.contains(&e.ext))
            && self.sizes.iter().all(|&(lo, hi)| lo <= e.size && e.size < hi)
            && self.dates.iter().all(|&(lo, hi)| lo <= e.modified && e.modified < hi)
    }
}

// ---- size: ----

const KB: u64 = 1024;
const MB: u64 = 1024 * KB;
const GB: u64 = 1024 * MB;

/// `10mb` → (10MB のバイト数, 単位のバイト数)
fn parse_size_value(s: &str) -> Option<(u64, u64)> {
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().ok()?;
    let unit = match unit {
        "" | "b" => 1,
        "k" | "kb" => KB,
        "m" | "mb" => MB,
        "g" | "gb" => GB,
        _ => return None,
    };
    Some(((n * unit as f64).round() as u64, unit))
}

fn parse_size_cond(v: &str) -> Option<(u64, u64)> {
    let named = match v {
        "empty" => Some((0, 1)),
        "tiny" => Some((0, 10 * KB)),
        "small" => Some((10 * KB, 100 * KB)),
        "medium" => Some((100 * KB, MB)),
        "large" => Some((MB, 16 * MB)),
        "huge" => Some((16 * MB, 128 * MB)),
        "gigantic" => Some((128 * MB, u64::MAX)),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if let Some((a, b)) = v.split_once("..") {
        let (lo, _) = parse_size_value(a)?;
        let (hi, _) = parse_size_value(b)?;
        return Some((lo, hi.saturating_add(1)));
    }
    for (op, f) in [
        (">=", 0u8),
        ("<=", 1),
        (">", 2),
        ("<", 3),
        ("=", 4),
    ] {
        if let Some(rest) = v.strip_prefix(op) {
            let (x, unit) = parse_size_value(rest)?;
            return Some(match f {
                0 => (x, u64::MAX),
                1 => (0, x.saturating_add(1)),
                2 => (x.saturating_add(1), u64::MAX),
                3 => (0, x),
                _ => (x, x.saturating_add(unit)),
            });
        }
    }
    let (x, unit) = parse_size_value(v)?;
    Some((x, x.saturating_add(unit)))
}

// ---- dm: ----

fn day_start(d: NaiveDate) -> i64 {
    let midnight = d.and_hms_opt(0, 0, 0).expect("0:00:00 は常に有効");
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .map_or_else(|| midnight.and_utc().timestamp(), |t| t.timestamp())
}

fn month_start(year: i32, month: u32) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(year, month, 1)
}

fn next_month(d: NaiveDate) -> Option<NaiveDate> {
    if d.month() == 12 {
        month_start(d.year() + 1, 1)
    } else {
        month_start(d.year(), d.month() + 1)
    }
}

/// `2026/09/01` `2026-09` `2026` を [その日/月/年の始まり, 終わり) にする
fn parse_date(s: &str) -> Option<(i64, i64)> {
    let parts: Vec<&str> = s.split(['/', '-']).collect();
    let nums: Vec<u32> = parts.iter().map(|p| p.parse().ok()).collect::<Option<_>>()?;
    match nums.as_slice() {
        [y] if parts[0].len() == 4 => {
            let y = *y as i32;
            Some((
                day_start(NaiveDate::from_ymd_opt(y, 1, 1)?),
                day_start(NaiveDate::from_ymd_opt(y + 1, 1, 1)?),
            ))
        }
        [y, m] => {
            let start = month_start(*y as i32, *m)?;
            Some((day_start(start), day_start(next_month(start)?)))
        }
        [y, m, d] => {
            let date = NaiveDate::from_ymd_opt(*y as i32, *m, *d)?;
            Some((day_start(date), day_start(date.succ_opt()?)))
        }
        _ => None,
    }
}

fn parse_date_cond(v: &str, now: DateTime<Local>) -> Option<(i64, i64)> {
    let today = now.date_naive();
    let week_start = today - Duration::days(today.weekday().num_days_from_sunday() as i64);
    let this_month = month_start(today.year(), today.month())?;
    let last_month = if today.month() == 1 {
        month_start(today.year() - 1, 12)?
    } else {
        month_start(today.year(), today.month() - 1)?
    };
    let year_start = |y: i32| NaiveDate::from_ymd_opt(y, 1, 1);
    let span = |a: NaiveDate, b: NaiveDate| Some((day_start(a), day_start(b)));
    match v {
        "today" => return span(today, today.succ_opt()?),
        "yesterday" => return span(today.pred_opt()?, today),
        "thisweek" => return span(week_start, week_start + Duration::days(7)),
        "lastweek" => return span(week_start - Duration::days(7), week_start),
        "thismonth" => return span(this_month, next_month(this_month)?),
        "lastmonth" => return span(last_month, this_month),
        "thisyear" => return span(year_start(today.year())?, year_start(today.year() + 1)?),
        "lastyear" => return span(year_start(today.year() - 1)?, year_start(today.year())?),
        _ => {}
    }
    if let Some(rest) = v.strip_prefix("last").or_else(|| v.strip_prefix("past")) {
        let split = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let (num, unit) = rest.split_at(split);
        let n: i64 = if num.is_empty() { 1 } else { num.parse().ok()? };
        let secs = match unit.trim_end_matches('s') {
            "min" | "minute" => 60,
            "hour" => 3600,
            "day" => 86_400,
            "week" => 7 * 86_400,
            "month" => 30 * 86_400,
            "year" => 365 * 86_400,
            _ => return None,
        };
        return Some((now.timestamp() - n * secs, i64::MAX));
    }
    if let Some((a, b)) = v.split_once("..") {
        let (lo, _) = parse_date(a)?;
        let (_, hi) = parse_date(b)?;
        return Some((lo, hi));
    }
    for (op, f) in [(">=", 0u8), ("<=", 1), (">", 2), ("<", 3), ("=", 4)] {
        if let Some(rest) = v.strip_prefix(op) {
            let (lo, hi) = parse_date(rest)?;
            return Some(match f {
                0 => (lo, i64::MAX),
                1 => (i64::MIN, hi),
                2 => (hi, i64::MAX),
                3 => (i64::MIN, lo),
                _ => (lo, hi),
            });
        }
    }
    parse_date(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-23（水）14:30 ローカル
    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 9, 23, 14, 30, 0).single().unwrap()
    }

    fn ts(y: i32, m: u32, d: u32) -> i64 {
        day_start(NaiveDate::from_ymd_opt(y, m, d).unwrap())
    }

    #[test]
    fn splits_filters_from_fuzzy_text() {
        let q = Query::parse("雨 ext:WAV;.mp3 size:>1mb 音", now());
        assert_eq!(q.text, "雨 音");
        assert_eq!(q.exts.len(), 1);
        assert!(q.exts[0].contains("wav") && q.exts[0].contains("mp3"));
        assert_eq!(q.sizes, vec![(MB + 1, u64::MAX)]);
        assert!(q.errors.is_empty());
    }

    #[test]
    fn size_forms() {
        assert_eq!(parse_size_cond(">=10kb"), Some((10 * KB, u64::MAX)));
        assert_eq!(parse_size_cond("<500"), Some((0, 500)));
        assert_eq!(parse_size_cond("<=1k"), Some((0, KB + 1)));
        assert_eq!(parse_size_cond("1mb..2mb"), Some((MB, 2 * MB + 1)));
        assert_eq!(parse_size_cond("10mb"), Some((10 * MB, 11 * MB)));
        assert_eq!(parse_size_cond("1.5mb"), Some((MB + MB / 2, 2 * MB + MB / 2)));
        assert_eq!(parse_size_cond("large"), Some((MB, 16 * MB)));
        assert_eq!(parse_size_cond("10xb"), None);
        assert_eq!(parse_size_cond(">"), None);
    }

    #[test]
    fn date_keywords_use_local_calendar() {
        let n = now();
        assert_eq!(parse_date_cond("today", n), Some((ts(2026, 9, 23), ts(2026, 9, 24))));
        assert_eq!(parse_date_cond("yesterday", n), Some((ts(2026, 9, 22), ts(2026, 9, 23))));
        // 2026-09-23 は水曜。週は日曜（9/20）から
        assert_eq!(parse_date_cond("thisweek", n), Some((ts(2026, 9, 20), ts(2026, 9, 27))));
        assert_eq!(parse_date_cond("lastweek", n), Some((ts(2026, 9, 13), ts(2026, 9, 20))));
        assert_eq!(parse_date_cond("thismonth", n), Some((ts(2026, 9, 1), ts(2026, 10, 1))));
        assert_eq!(parse_date_cond("lastmonth", n), Some((ts(2026, 8, 1), ts(2026, 9, 1))));
        assert_eq!(parse_date_cond("lastyear", n), Some((ts(2025, 1, 1), ts(2026, 1, 1))));
    }

    #[test]
    fn date_relative_and_absolute() {
        let n = now();
        assert_eq!(parse_date_cond("last3days", n), Some((n.timestamp() - 3 * 86_400, i64::MAX)));
        assert_eq!(parse_date_cond("past2hours", n), Some((n.timestamp() - 7200, i64::MAX)));
        assert_eq!(parse_date_cond("2026/09/01", n), Some((ts(2026, 9, 1), ts(2026, 9, 2))));
        assert_eq!(parse_date_cond(">=2026-09", n), Some((ts(2026, 9, 1), i64::MAX)));
        assert_eq!(parse_date_cond("<2026", n), Some((i64::MIN, ts(2026, 1, 1))));
        assert_eq!(
            parse_date_cond("2026/09/01..2026/09/15", n),
            Some((ts(2026, 9, 1), ts(2026, 9, 16)))
        );
        assert_eq!(parse_date_cond("2026/13/01", n), None);
        assert_eq!(parse_date_cond("someday", n), None);
    }

    #[test]
    fn invalid_terms_are_reported_and_ignored() {
        let q = Query::parse("dm:someday size:big 雨", now());
        assert_eq!(q.text, "雨");
        assert!(q.dates.is_empty() && q.sizes.is_empty());
        assert_eq!(q.errors, vec!["dm:someday", "size:big"]);
    }

    #[test]
    fn matches_attrs_combines_with_and() {
        let e = crate::index::make_entry(
            "C:/a/rain.wav".into(),
            "rain.wav".into(),
            "a".into(),
            "wav".into(),
            2 * MB,
            ts(2026, 9, 23) + 100,
        );
        assert!(Query::parse("ext:wav size:>1mb dm:today", now()).matches_attrs(&e));
        assert!(!Query::parse("ext:png", now()).matches_attrs(&e));
        assert!(!Query::parse("size:<1mb", now()).matches_attrs(&e));
        assert!(!Query::parse("dm:yesterday", now()).matches_attrs(&e));
    }
}
