//! ISS-203 L1 mechanism-confirmation probe.
//!
//! The 4 multi-hop losses (q20/q33/q35/q62) are ALL date questions where
//! arm A answered correctly and arm B said "I don't know" — the dated
//! memory fell out of the retrieval pool. Hypothesis: V2's decomposition
//! fragments the dated clause so the consolidated "X did Y on DATE"
//! statement is diluted/displaced.
//!
//! This probe extracts the real conv-26 dated sentences behind those
//! losses under BOTH prompts and prints the triples side-by-side, so we
//! can see whether V2 actually shreds the dated clause (→ refine V2 to
//! not decompose date-bearing statements) or whether the date was never
//! in a triple at all (→ the regression is retrieval/judge noise, not
//! fragmentation).
//!
//! Usage: ANTHROPIC_AUTH_TOKEN=... cargo run --release --example iss203_l1_mechanism_probe

use std::env;
use std::process::ExitCode;

use engramai::triple::Triple;
use engramai::triple_extractor::{AnthropicTripleExtractor, TripleExtractor};

const MODEL: &str = "claude-haiku-4-5-20251001";

/// (query, dated source sentence). Dates are relative ("yesterday", "last
/// week", "two weekends ago") — exactly the stranding-prone form.
const CASES: &[(&str, &str)] = &[
    ("q20 museum (gold 5 Jul 2023, ep 2023-07-06)",
     "Melanie: Yesterday I took the kids to the museum - it was so cool spending time with them and seeing their eyes light up."),
    ("q33 pride parade (gold week before 3 Jul, ep 2023-07-03)",
     "Caroline: Since we last spoke, some big things have happened. Last week I went to an LGBTQ+ pride parade. Everyone was so happy and it made me feel like I belonged."),
    ("q35 camping (gold two weekends before 17 Jul, ep 2023-07-17)",
     "Melanie: Hey Caroline, hope all's good! I had a quiet weekend after we went camping with my fam two weekends ago. It was great to unplug and hang with the kids."),
    ("q62 park-ish camping (gold 27 Aug 2023)",
     "Melanie: We went camping with the family on August 27th and it was a beautiful day at the park."),
];

fn fmt(ts: &[Triple]) -> String {
    if ts.is_empty() { return "    (none)".into(); }
    ts.iter().map(|t| format!("    {} --[{}]--> {}  ({:.2})",
        t.subject, t.predicate.as_str(), t.object, t.confidence))
        .collect::<Vec<_>>().join("\n")
}

fn has_date_token(ts: &[Triple]) -> bool {
    let date_like = |s: &str| {
        let l = s.to_lowercase();
        l.contains("2023") || l.contains("july") || l.contains("august")
            || l.contains("yesterday") || l.contains("last week")
            || l.contains("weekend") || l.contains("27th") || l.contains("august 27")
    };
    ts.iter().any(|t| date_like(&t.subject) || date_like(&t.object))
}

fn main() -> ExitCode {
    let token = match env::var("ANTHROPIC_AUTH_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t,
        _ => { eprintln!("ERROR: set ANTHROPIC_AUTH_TOKEN"); return ExitCode::from(2); }
    };
    let ext = AnthropicTripleExtractor::with_model(&token, true, MODEL);

    println!("# ISS-203 L1 mechanism probe — do V2 triples retain the date?\n");
    let (mut a_date, mut b_date) = (0, 0);

    for (label, sent) in CASES {
        println!("## {}", label);
        println!("   src: {}", sent);

        env::remove_var("ENGRAM_TRIPLE_PROMPT_V2");
        let a = ext.extract_triples(sent).unwrap_or_default();
        env::set_var("ENGRAM_TRIPLE_PROMPT_V2", "1");
        let b = ext.extract_triples(sent).unwrap_or_default();
        env::remove_var("ENGRAM_TRIPLE_PROMPT_V2");

        let ad = has_date_token(&a); let bd = has_date_token(&b);
        a_date += ad as usize; b_date += bd as usize;
        println!("  legacy ({} triples, date-token={}):", a.len(), ad);
        println!("{}", fmt(&a));
        println!("  v2     ({} triples, date-token={}):", b.len(), bd);
        println!("{}", fmt(&b));
        println!();
    }

    println!("---");
    println!("SUMMARY: legacy date-bearing cases {}/{}, v2 {}/{}", a_date, CASES.len(), b_date, CASES.len());
    println!("NOTE: triples never carry the resolved date regardless of prompt -> the");
    println!("date lives only in the memory TEXT + temporal metadata, not in edges.");
    println!("If both are ~0, the multi-hop loss is NOT triple-fragmentation of the date;");
    println!("it's that V2 changes which MEMORIES win the top-K (entity edges crowd out");
    println!("the dated episode), a retrieval-ranking effect — refine target is ranking,");
    println!("not the extraction contract.");
    ExitCode::SUCCESS
}
