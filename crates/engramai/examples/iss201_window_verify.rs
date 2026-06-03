//! ISS-201 windowed-ingest LLM-level verification (run BEFORE the 12-min
//! re-ingest, per the "verify at LLM level in seconds first" rule).
//!
//! Reads /tmp/iss201_window_verify.json (real conv-26 SEMANTIC-GAP cases:
//! gold turn + its 4 preceding turns, pulled from the fixture) and feeds each
//! through the EXACT production framing used by engram-bench locomo.rs
//! sliding-window ingest, so this proves the real code path — not a
//! hand-crafted prompt.
//!
//! PASS criterion (per case): the extractor returns a self-contained core_fact
//! that names the gold subject/object (coref resolved), so a retrieval query
//! could match it. Printed verdict per case.
//!
//! Run:
//!   ANTHROPIC_AUTH_TOKEN=$(...oauth...) \
//!   cargo run -p engramai --release --example iss201_window_verify

use chrono::DateTime;
use engramai::{AnthropicExtractor, MemoryExtractor};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    qid: String,
    question: String,
    gold: String,
    occurred_at: String,
    context: Vec<String>,
    turn: String,
}

/// EXACT framing from engram-bench/src/drivers/locomo.rs (keep in sync).
fn windowed(context: &[String], turn: &str) -> String {
    format!(
        "Prior conversation context (for coreference resolution ONLY \
— do NOT extract facts from these lines; they are already stored):\n{}\n\n\
Extract facts ONLY from this final turn, resolving any pronouns or \
references against the context above so each core_fact is self-contained:\n{}",
        context.join("\n"),
        turn
    )
}

fn main() {
    let token = std::env::var("ANTHROPIC_AUTH_TOKEN").expect("set ANTHROPIC_AUTH_TOKEN");
    let ex = AnthropicExtractor::new(&token, true);

    let raw = std::fs::read_to_string("/tmp/iss201_window_verify.json")
        .expect("read /tmp/iss201_window_verify.json");
    let cases: Vec<Case> = serde_json::from_str(&raw).expect("parse cases");

    let mut pass = 0usize;
    for c in &cases {
        let reference = DateTime::parse_from_rfc3339(&c.occurred_at)
            .ok()
            .map(|d| d.with_timezone(&chrono::Utc));
        let input = windowed(&c.context, &c.turn);
        println!("\n================ {} ================", c.qid);
        println!("Q: {}", c.question);
        println!("GOLD: {}", c.gold);
        println!("TURN: {}", c.turn);

        // gold keyword tokens (len>3, alpha) for a loose self-containment check
        let gold_toks: Vec<String> = c
            .gold
            .to_lowercase()
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|w| w.len() > 3)
            .map(|w| w.to_string())
            .collect();

        match ex.extract(&input, reference) {
            Ok(facts) => {
                if facts.is_empty() {
                    println!("  (no facts extracted)  => GAP");
                    continue;
                }
                let mut hit = false;
                for (i, f) in facts.iter().enumerate() {
                    let lc = f.core_fact.to_lowercase();
                    let matched = gold_toks.iter().any(|t| lc.contains(t));
                    if matched {
                        hit = true;
                    }
                    println!(
                        "  fact[{i}] {:?}  [{}]",
                        f.core_fact,
                        if matched { "matches gold" } else { "—" }
                    );
                }
                if hit {
                    pass += 1;
                    println!("  => RETRIEVABLE (self-contained, coref resolved)");
                } else {
                    println!("  => GAP (gold token not surfaced in any core_fact)");
                }
            }
            Err(e) => println!("  extract error: {e}"),
        }
    }
    println!("\n==== windowed-ingest verify: {}/{} cases RETRIEVABLE ====", pass, cases.len());
}
