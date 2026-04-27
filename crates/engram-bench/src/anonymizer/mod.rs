//! Anonymizer for the rustclaw production-trace fixture (design §9.3.1).
//!
//! Implements the one-shot precondition specified in design §9.3.1: scrubs
//! PII / proprietary content from a captured rustclaw trace before it can
//! be committed as fixture material.
//!
//! **Zero-leak rule** (GOAL-5.4 + GUARD on fixture safety): anonymizer
//! output must pass a re-identification audit before any commit. The
//! [`Anonymizer::check_no_leak`] helper (sub-task 2) is the programmatic
//! line of defence; the §9.3.1 potato-review of `diff.txt` is the human
//! line of defence.
//!
//! ## Determinism contract
//!
//! Per design §9.3.1: same input + same catalogs + same delta ⇒
//! byte-identical output. The pipeline is therefore a strictly ordered
//! composition:
//!
//! 1. For each pattern in `patterns` (in declared order), apply
//!    `Regex::replace_all` with the pattern's typed replacement template.
//! 2. For each `(from, to)` pair in `delta`, do a literal string
//!    substitution (BTreeMap iteration → deterministic order).
//!
//! Allowlist entries are removed from candidate match spans before
//! replacement so legitimate tokens (`engram`, `RustClaw`) survive.
//!
//! ## Failure semantics
//!
//! All-or-nothing per design §9.3.1 "Failure handling": any malformed
//! regex in `patterns.toml` ⇒ [`Anonymizer::from_config_files`] returns
//! `BenchError::Other` and the caller aborts. Partial state is never
//! returned; partial corpora are never written. Idempotence checking
//! lives in sub-task 2 ([`Anonymizer::is_idempotent`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::harness::BenchError;

/// Single named replacement rule loaded from `patterns.toml`.
///
/// Each rule combines a regex (compiled once at load time) with a typed
/// replacement template — e.g. `(r"[\w.+-]+@[\w-]+(\.[\w-]+)+", "<EMAIL>")`.
/// The replacement string is interpolated by `Regex::replace_all` using
/// the standard `$0`/`$1`/… capture syntax, so callers can keep capture
/// groups in their patterns and reference them in the template.
#[derive(Debug, Clone)]
pub struct PatternRule {
    /// Human-readable name for diagnostics (`email`, `url`, `person`).
    /// Echoed in error messages when a regex fails to compile.
    pub name: String,
    /// Compiled regex.
    pub regex: Regex,
    /// Replacement template. May contain backreferences like `$0`.
    pub replacement: String,
}

/// Wire-format of one row in `patterns.toml`. Lifted into [`PatternRule`]
/// (with a compiled `Regex`) by [`Anonymizer::from_config_files`].
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PatternToml {
    name: String,
    regex: String,
    replacement: String,
}

/// Wire-format of `patterns.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PatternsFile {
    #[serde(default)]
    pattern: Vec<PatternToml>,
}

/// Wire-format of `allowlist.toml` (single `tokens` array).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AllowlistFile {
    #[serde(default)]
    tokens: Vec<String>,
}

/// Wire-format of `delta.toml` — literal string substitutions applied
/// after the regex pipeline. Stored as a TOML inline table so the
/// committed file reads as `from_a = "to_a"` per pair.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct DeltaFile {
    #[serde(default)]
    substitutions: BTreeMap<String, String>,
}

/// Deterministic regex + allow-list anonymizer (design §9.3.1).
///
/// Construction is fallible (any bad regex aborts the load); application
/// is infallible — once an `Anonymizer` exists, [`Self::apply`] cannot
/// fail. This keeps the all-or-nothing contract honest: every per-episode
/// run either succeeds or the whole pass is rerun from scratch.
#[derive(Debug, Clone)]
pub struct Anonymizer {
    /// Regex rules, applied in declared order. `Vec` (not `HashMap`) to
    /// preserve ordering across runs — required for byte-identical output.
    pub patterns: Vec<PatternRule>,
    /// Tokens that look like PII but aren't (e.g. `RustClaw`, `engram`).
    /// Spans matching one of these are restored verbatim. `BTreeSet` for
    /// stable iteration order.
    pub allowlist: BTreeSet<String>,
    /// Literal substitutions applied **after** the regex pipeline.
    /// `BTreeMap` for stable iteration order across runs.
    pub delta: BTreeMap<String, String>,
}

impl Anonymizer {
    /// Load all three config files and compile the regex catalog.
    ///
    /// Per design §9.3.1: all three files are required artifacts under
    /// `benchmarks/fixtures/rustclaw_trace/anonymizer/`. Missing files,
    /// malformed TOML, or any single bad regex abort the load with
    /// `BenchError::Other`.
    pub fn from_config_files(
        patterns_toml: &Path,
        allowlist_toml: &Path,
        delta_toml: &Path,
    ) -> Result<Self, BenchError> {
        let patterns_body = fs::read_to_string(patterns_toml)?;
        let patterns_file: PatternsFile = toml::from_str(&patterns_body).map_err(|e| {
            BenchError::Other(format!(
                "anonymizer: parsing {}: {e}",
                patterns_toml.display()
            ))
        })?;

        let allowlist_body = fs::read_to_string(allowlist_toml)?;
        let allowlist_file: AllowlistFile = toml::from_str(&allowlist_body).map_err(|e| {
            BenchError::Other(format!(
                "anonymizer: parsing {}: {e}",
                allowlist_toml.display()
            ))
        })?;

        let delta_body = fs::read_to_string(delta_toml)?;
        let delta_file: DeltaFile = toml::from_str(&delta_body).map_err(|e| {
            BenchError::Other(format!(
                "anonymizer: parsing {}: {e}",
                delta_toml.display()
            ))
        })?;

        let mut patterns = Vec::with_capacity(patterns_file.pattern.len());
        for raw in patterns_file.pattern {
            let regex = Regex::new(&raw.regex).map_err(|e| {
                BenchError::Other(format!(
                    "anonymizer: compiling pattern `{}` (`{}`): {e}",
                    raw.name, raw.regex
                ))
            })?;
            patterns.push(PatternRule {
                name: raw.name,
                regex,
                replacement: raw.replacement,
            });
        }

        Ok(Self {
            patterns,
            allowlist: allowlist_file.tokens.into_iter().collect(),
            delta: delta_file.substitutions,
        })
    }

    /// Run the anonymization pipeline on a string.
    ///
    /// Order of operations (per the determinism contract above):
    /// 1. For each rule in `patterns` (in declared order): replace every
    ///    non-allowlisted match span with the rule's replacement template.
    /// 2. For each `(from, to)` in `delta` (BTreeMap order): literal
    ///    substring substitution.
    ///
    /// Allowlist semantics: a captured span equal (case-sensitive) to an
    /// allowlist entry is **not** replaced; the original substring is
    /// preserved. Other matches are replaced as usual.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for rule in &self.patterns {
            let allowlist = &self.allowlist;
            let replacement = &rule.replacement;
            // `Regex::replace_all` with a closure: per-match decision.
            out = rule
                .regex
                .replace_all(&out, |caps: &regex::Captures<'_>| -> String {
                    let matched = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                    if allowlist.contains(matched) {
                        // Preserve allowlisted token verbatim.
                        return matched.to_owned();
                    }
                    // Standard backreference expansion ($0, $1, …).
                    let mut buf = String::new();
                    caps.expand(replacement, &mut buf);
                    buf
                })
                .into_owned();
        }
        for (from, to) in &self.delta {
            if !from.is_empty() {
                out = out.replace(from, to);
            }
        }
        out
    }

    /// Verify that running the anonymizer twice on the same input
    /// produces byte-identical output.
    ///
    /// Per design §9.3.1 idempotence test (cross-linked from §11): the
    /// committed catalog must satisfy `apply(apply(x)) == apply(x)` for
    /// every input. Catalog regressions that introduce a non-idempotent
    /// rule (e.g. a pattern that matches its own replacement marker) are
    /// caught here.
    ///
    /// This is **not** a property test — it answers the idempotence
    /// question for one specific input. CI runs it across the full
    /// committed corpus to validate the catalog (per §11).
    pub fn is_idempotent(&self, text: &str) -> bool {
        let once = self.apply(text);
        let twice = self.apply(&once);
        once == twice
    }

    /// Confirm that `text` contains none of `banned_substrings`.
    ///
    /// Returns `Ok(())` on a clean scan. Returns `Err(Vec<String>)` with
    /// the list of leaked substrings (each appears at most once in the
    /// returned vector even if it occurs multiple times in `text`).
    ///
    /// Per design §9.3.1 "Acceptable leak tolerance: zero" — this
    /// programmatic line of defence runs alongside the human review of
    /// `diff.txt`. Comparison is case-sensitive: the typed entity
    /// markers (`<EMAIL>`, `<PERSON_1>`) are deliberately mixed-case so
    /// case-sensitive matching catches genuine PII while preserving
    /// legitimate camelCase code identifiers.
    pub fn check_no_leak(
        &self,
        text: &str,
        banned_substrings: &[&str],
    ) -> Result<(), Vec<String>> {
        let mut leaks: Vec<String> = Vec::new();
        for banned in banned_substrings {
            if banned.is_empty() {
                continue;
            }
            if text.contains(*banned) && !leaks.iter().any(|l| l == *banned) {
                leaks.push((*banned).to_string());
            }
        }
        if leaks.is_empty() {
            Ok(())
        } else {
            Err(leaks)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: write three TOML files into `dir` and load an `Anonymizer`
    /// from them. Returns the loaded instance for further assertions.
    fn load_from_strings(
        dir: &Path,
        patterns: &str,
        allowlist: &str,
        delta: &str,
    ) -> Result<Anonymizer, BenchError> {
        let p = dir.join("patterns.toml");
        let a = dir.join("allowlist.toml");
        let d = dir.join("delta.toml");
        fs::write(&p, patterns).expect("seed patterns");
        fs::write(&a, allowlist).expect("seed allowlist");
        fs::write(&d, delta).expect("seed delta");
        Anonymizer::from_config_files(&p, &a, &d)
    }

    /// Test 1: the regex pipeline replaces matches and leaves
    /// allowlisted tokens untouched. Anchors the §9.3.1 spec example
    /// (email → `<EMAIL>`, allowlisted project name preserved).
    #[test]
    fn applies_patterns_and_respects_allowlist() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let patterns = r#"
[[pattern]]
name = "email"
regex = '[\w.+-]+@[\w.-]+\.[A-Za-z]+'
replacement = "<EMAIL>"

[[pattern]]
name = "project_name"
regex = 'engram'
replacement = "<PROJECT>"
"#;
        let allowlist = r#"
tokens = ["engram"]
"#;
        let delta = r#"
[substitutions]
"#;
        let anon = load_from_strings(tmp.path(), patterns, allowlist, delta).expect("load");

        let out = anon.apply("Contact alice@example.com about the engram crate.");
        assert_eq!(
            out,
            "Contact <EMAIL> about the engram crate.",
            "email replaced; allowlisted `engram` survives"
        );
    }

    /// Test 2: literal `delta` substitutions run after the regex pipeline,
    /// in deterministic (BTreeMap) order. Anchors the determinism contract
    /// — same delta map ⇒ same output.
    #[test]
    fn applies_delta_after_patterns_in_btree_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let patterns = r#"
[[pattern]]
name = "name"
regex = 'Alice'
replacement = "<PERSON_1>"
"#;
        let allowlist = r#"tokens = []"#;
        let delta = r#"
[substitutions]
"<PERSON_1>" = "<P1>"
"work" = "WORK"
"#;
        let anon = load_from_strings(tmp.path(), patterns, allowlist, delta).expect("load");

        // Pipeline: "Alice" → "<PERSON_1>" → "<P1>"; "work" → "WORK".
        // Whole transform is byte-identical on repeat application of the
        // same input, exercising the determinism property indirectly.
        let out1 = anon.apply("Alice does work.");
        let out2 = anon.apply("Alice does work.");
        assert_eq!(out1, "<P1> does WORK.");
        assert_eq!(out1, out2, "deterministic — same input ⇒ same output");
    }

    /// Test 3: any malformed regex in `patterns.toml` aborts the load with
    /// a `BenchError::Other` naming the offending pattern. Per §9.3.1
    /// all-or-nothing semantics — partial loads are forbidden.
    #[test]
    fn malformed_regex_aborts_load_with_named_diagnostic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let patterns = r#"
[[pattern]]
name = "broken"
regex = '([unclosed'
replacement = "<X>"
"#;
        let allowlist = r#"tokens = []"#;
        let delta = r#"[substitutions]"#;

        let err = load_from_strings(tmp.path(), patterns, allowlist, delta)
            .expect_err("must fail on malformed regex");
        let BenchError::Other(msg) = err else { panic!("wrong variant") };
        assert!(
            msg.contains("compiling pattern `broken`"),
            "diagnostic must name the offending pattern, got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // Sub-task 2 tests: is_idempotent + check_no_leak (§9.3.1, §11).
    // -----------------------------------------------------------------------

    /// Test 4: idempotence on a clean input — applying the anonymizer to
    /// already-anonymized text is a no-op.
    #[test]
    fn idempotent_on_already_anonymized_input() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let patterns = r#"
[[pattern]]
name = "email"
regex = '[\w.+-]+@[\w.-]+\.[A-Za-z]+'
replacement = "<EMAIL>"
"#;
        let allowlist = r#"tokens = []"#;
        let delta = r#"[substitutions]"#;
        let anon = load_from_strings(tmp.path(), patterns, allowlist, delta).expect("load");

        // After one pass, no email remains; the typed marker `<EMAIL>`
        // does not match the email pattern, so a second pass is a no-op.
        assert!(anon.is_idempotent("Mail alice@example.com today"));
        assert!(anon.is_idempotent("clean text without PII"));
    }

    /// Test 5: idempotence holds even when `delta` introduces a marker
    /// that doesn't re-trigger any pattern. Catalog authors must avoid
    /// rules that match their own output — this test fails loudly when
    /// they don't.
    #[test]
    fn idempotent_after_first_pass_under_delta_rewrites() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let patterns = r#"
[[pattern]]
name = "person"
regex = 'Alice'
replacement = "<PERSON_1>"
"#;
        let allowlist = r#"tokens = []"#;
        let delta = r#"
[substitutions]
"<PERSON_1>" = "<P1>"
"#;
        let anon = load_from_strings(tmp.path(), patterns, allowlist, delta).expect("load");

        // First pass: "Alice" → "<PERSON_1>" → "<P1>". Second pass: no
        // remaining "Alice" or "<PERSON_1>" to rewrite ⇒ identity.
        assert!(anon.is_idempotent("Alice was here"));
    }

    /// Test 6: `check_no_leak` flags banned substrings that survived.
    /// Returns the offending tokens so the human review step (§9.3.1
    /// `diff.txt` audit) knows precisely what failed.
    #[test]
    fn check_no_leak_flags_banned_substrings() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let patterns = r#"[[pattern]]
name = "noop"
regex = 'NEVER_MATCHES_ZZZZZZ'
replacement = "<X>"
"#;
        let allowlist = r#"tokens = []"#;
        let delta = r#"[substitutions]"#;
        let anon = load_from_strings(tmp.path(), patterns, allowlist, delta).expect("load");

        let banned = ["potato@example.com", "secret_api_key"];
        let polluted = "contact potato@example.com using secret_api_key";
        let err = anon
            .check_no_leak(polluted, &banned)
            .expect_err("must report leaks");
        // Order matches scan order over banned slice.
        assert_eq!(err, vec!["potato@example.com", "secret_api_key"]);
    }

    /// Test 7: clean text passes `check_no_leak`. Anchors the zero-leak
    /// invariant — the function returns `Ok(())` only when no banned
    /// substring is present.
    #[test]
    fn check_no_leak_passes_on_clean_text() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let patterns = r#"[[pattern]]
name = "noop"
regex = 'NEVER_MATCHES_ZZZZZZ'
replacement = "<X>"
"#;
        let allowlist = r#"tokens = []"#;
        let delta = r#"[substitutions]"#;
        let anon = load_from_strings(tmp.path(), patterns, allowlist, delta).expect("load");

        let clean = "<PERSON_1> sent <EMAIL> regarding <URL>";
        let banned = ["alice@example.com", "Bob Smith"];
        anon.check_no_leak(clean, &banned).expect("clean text must pass");
    }
}
