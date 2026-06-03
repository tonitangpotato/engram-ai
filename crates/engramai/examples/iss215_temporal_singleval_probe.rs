//! ISS-215 manual pre-ingest verification: does the FIXED extractor prompt
//! (commit 122d964c) emit ONE clean canonical temporal value instead of a
//! concatenated multi-signal string?
//!
//! Baseline failure (q8, old prompt): the extractor produced
//!   "last week (2023-05-29) (week of ~2023-06-02), three years into
//!    transition (started ~2020)"
//! for a sentence that mixed a relative event date with a duration cue.
//!
//! Run:
//!   ANTHROPIC_AUTH_TOKEN=$(... oauth ...) \
//!   cargo run -p engramai --release --example iss215_temporal_singleval_probe

use chrono::{TimeZone, Utc};
use engramai::{AnthropicExtractor, MemoryExtractor};

fn main() {
    let token = std::env::var("ANTHROPIC_AUTH_TOKEN").expect("set ANTHROPIC_AUTH_TOKEN");
    let ex = AnthropicExtractor::new(&token, true);

    // Reference date stands in for the conversation turn's occurred_at.
    let reference = Utc.with_ymd_and_hms(2023, 6, 2, 12, 0, 0).unwrap();

    // Each case mixes MULTIPLE time cues in one event — the exact shape that
    // made the old prompt concatenate everything. The expected output is a
    // SINGLE bare value, no brackets/parens/notes.
    let cases: &[(&str, &str)] = &[
        (
            "q8-style: relative event date + duration backstory",
            "Caroline: Last week I gave a talk at a school event about my \
             transgender journey and encouraged students to get involved in \
             the LGBTQ community. I've been three years into my transition now.",
        ),
        (
            "duration-only (year granularity expected)",
            "Caroline: I've been volunteering at the youth center for seven \
             years and it's the most rewarding thing I do.",
        ),
        (
            "ongoing aspect, no concrete date (expect a single aspect keyword)",
            "Caroline: I'm seriously considering pursuing counseling and \
             mental health as a career path to give back.",
        ),
        (
            "two absolute-ish cues in one event",
            "Caroline: We had the adoption meeting the Friday before last, \
             and we'd been on the waitlist since early 2021.",
        ),
    ];

    for (label, text) in cases {
        println!("\n=== {label} ===");
        println!("input: {text}");
        match ex.extract(text, Some(reference)) {
            Ok(facts) => {
                if facts.is_empty() {
                    println!("  (no facts extracted)");
                }
                for (i, f) in facts.iter().enumerate() {
                    println!(
                        "  fact[{i}] core_fact={:?}",
                        f.core_fact.chars().take(70).collect::<String>()
                    );
                    println!("           temporal={:?}", f.temporal);
                    // Heuristic verdict: a clean single value has no comma-list,
                    // no bracket, and at most one parenthesis-free token group.
                    if let Some(t) = &f.temporal {
                        let noisy = t.contains('[')
                            || t.contains(']')
                            || t.matches('(').count() > 0
                            || t.contains(", ");
                        println!(
                            "           VERDICT: {}",
                            if noisy { "NOISY (multi-signal/parens)" } else { "CLEAN (single value)" }
                        );
                    }
                }
            }
            Err(e) => println!("  extract error: {e}"),
        }
    }
}
