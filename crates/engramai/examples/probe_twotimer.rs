// Probe: what does two_timer return for q29's actual stored temporal value?
use chrono::{NaiveDate, NaiveDateTime};

fn try_parse(phrase: &str, now: NaiveDateTime) {
    let config = two_timer::Config::new().now(now);
    match two_timer::parse(phrase, Some(config)) {
        Ok((start, end, is_range)) => {
            println!("  {:?}\n    -> Ok start={} end={} is_range={}", phrase, start, end, is_range);
        }
        Err(e) => {
            println!("  {:?}\n    -> Err({:?})", phrase, e);
        }
    }
}

fn main() {
    // occurred_at = 1679922600.0 = 2023-03-27 14:30:00 UTC (the memory's reference)
    let now = NaiveDate::from_ymd_opt(2023, 3, 27).unwrap().and_hms_opt(14, 30, 0).unwrap();
    println!("reference now = {}", now);
    println!("--- the value ACTUALLY stored in q29 metadata ---");
    try_parse("3 years (duration)", now);
    println!("--- variants ---");
    try_parse("3 years", now);
    try_parse("3 years ago", now);
    try_parse("for 3 years", now);
    try_parse("owned for 3 years", now);
    try_parse("since 3 years ago", now);
    println!("--- sanity: things that SHOULD parse ---");
    try_parse("yesterday", now);
    try_parse("last year", now);
    try_parse("2 months ago", now);
}
