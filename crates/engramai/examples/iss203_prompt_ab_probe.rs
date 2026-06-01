//! ISS-203 — pre-sweep prompt A/B probe.
//!
//! Cheap gate BEFORE the 2.5-3h conv-26+conv-44 sweep: feed real conv-26
//! sentences (with possessive / prepositional phrases) through the LIVE
//! Haiku extractor under both prompts and eyeball whether V2 actually
//! changes LLM behavior in the intended way.
//!
//! The unit tests only assert the prompt *string* contains the contract.
//! This probe asserts the *LLM* honors it. ISS-161/162/178 proved prompt
//! edits often don't change behavior (or change it for the worse), so we
//! verify on real text before burning the sweep budget.
//!
//! Usage:
//! ```bash
//! ANTHROPIC_AUTH_TOKEN=sk-ant-... \
//!   cargo run --release --example iss203_prompt_ab_probe
//! ```
//!
//! Reads ENGRAM_TRIPLE_PROMPT_V2 internally by toggling the env var around
//! each extract call, so a single binary run produces both arms.

use std::env;
use std::process::ExitCode;

use engramai::triple::Triple;
use engramai::triple_extractor::{AnthropicTripleExtractor, TripleExtractor};

const MODEL: &str = "claude-haiku-4-5-20251001";

/// Real conv-26 sentences chosen to exercise the possessive / prepositional
/// phrase failure mode. Each should, under V2, decompose into atomic
/// entities + a relation rather than emitting the phrase as one entity.
const SENTENCES: &[&str] = &[
    "Caroline showed her paintings at the LGBTQ art show",
    "Caroline attended a LGBTQ support group",
    "Caroline found transgender stories inspiring and felt happy and thankful for the support received",
    "Caroline feels accepted by the support group and has gained courage to embrace herself",
    "Caroline is interested in pursuing counseling or mental health work to support people with similar issues to her own.",
    "Melanie is taking care of the kids and going swimming with them",
    "Caroline read the book Becoming Nicole about a transgender girl",
    "Melanie loves Caroline's painting and finds the support group helpful",
];

fn fmt_triples(ts: &[Triple]) -> String {
    if ts.is_empty() {
        return "    (none)".to_string();
    }
    ts.iter()
        .map(|t| {
            format!(
                "    {} --[{}]--> {}  (conf {:.2})",
                t.subject,
                t.predicate.as_str(),
                t.object,
                t.confidence
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn main() -> ExitCode {
    let token = match env::var("ANTHROPIC_AUTH_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t,
        _ => {
            eprintln!("ERROR: set ANTHROPIC_AUTH_TOKEN (OAuth bearer)");
            return ExitCode::from(2);
        }
    };

    // OAuth bearer token => is_oauth=true.
    let extractor = AnthropicTripleExtractor::with_model(&token, true, MODEL);

    println!(
        "# ISS-203 prompt A/B probe ({} sentences, model {})\n",
        SENTENCES.len(),
        MODEL
    );

    let mut total_legacy_phrase_objs = 0usize;
    let mut total_v2_phrase_objs = 0usize;
    let mut total_v2_belongs_or_assoc = 0usize;

    for (i, sent) in SENTENCES.iter().enumerate() {
        println!("## [{}] {}", i + 1, sent);

        // --- LEGACY arm ---
        env::remove_var("ENGRAM_TRIPLE_PROMPT_V2");
        let legacy = match extractor.extract_triples(sent) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("  legacy extract error: {e}");
                Vec::new()
            }
        };

        // --- V2 arm ---
        env::set_var("ENGRAM_TRIPLE_PROMPT_V2", "1");
        let v2 = match extractor.extract_triples(sent) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("  v2 extract error: {e}");
                Vec::new()
            }
        };
        env::remove_var("ENGRAM_TRIPLE_PROMPT_V2");

        // Heuristic: a subject/object that contains a possessive marker
        // ("'s ", "'s") or a prepositional connector (" from ", " with ",
        // " of ") is a buried-relation phrase — the thing V2 should kill.
        let is_phrase = |s: &str| {
            let l = s.to_lowercase();
            l.contains("'s ")
                || l.ends_with("'s")
                || l.contains(" from ")
                || l.contains(" with ")
                || l.contains(" of ")
        };
        let count_phrase = |ts: &[Triple]| {
            ts.iter()
                .filter(|t| is_phrase(&t.subject) || is_phrase(&t.object))
                .count()
        };
        let legacy_phrase = count_phrase(&legacy);
        let v2_phrase = count_phrase(&v2);
        let v2_decomp = v2
            .iter()
            .filter(|t| matches!(t.predicate.as_str(), "part_of" | "related_to"))
            .count();

        total_legacy_phrase_objs += legacy_phrase;
        total_v2_phrase_objs += v2_phrase;
        total_v2_belongs_or_assoc += v2_decomp;

        println!(
            "  legacy ({} triples, {} phrase-endpoints):",
            legacy.len(),
            legacy_phrase
        );
        println!("{}", fmt_triples(&legacy));
        println!(
            "  v2     ({} triples, {} phrase-endpoints, {} part_of/related_to):",
            v2.len(),
            v2_phrase,
            v2_decomp
        );
        println!("{}", fmt_triples(&v2));
        println!();
    }

    println!("---");
    println!("SUMMARY:");
    println!(
        "  legacy phrase-endpoint triples total: {}",
        total_legacy_phrase_objs
    );
    println!(
        "  v2     phrase-endpoint triples total: {}",
        total_v2_phrase_objs
    );
    println!(
        "  v2     part_of/related_to triples total: {}",
        total_v2_belongs_or_assoc
    );
    println!();
    println!("GATE: V2 should have FEWER phrase-endpoint triples than legacy");
    println!("      AND produce part_of/related_to decomposition edges.");
    if total_v2_phrase_objs < total_legacy_phrase_objs && total_v2_belongs_or_assoc > 0 {
        println!("  => PASS (behavior changed in intended direction) — proceed to sweep");
    } else if total_legacy_phrase_objs == 0 && total_v2_phrase_objs == 0 {
        println!("  => INCONCLUSIVE (legacy already emitted no phrase endpoints on this sample)");
    } else {
        println!("  => FAIL or INERT — inspect diffs above BEFORE burning sweep budget");
    }

    ExitCode::SUCCESS
}
