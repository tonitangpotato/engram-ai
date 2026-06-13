//! Entity-centric aggregate-memory synthesis (ISS-201 lever-(b)).
//!
//! List/set questions ("what are X's hobbies/pets") fail in retrieval because
//! the answer items scatter across ranks and never co-locate in top-K. A
//! window-widening fix (global K) was shown to be a per-corpus overfit
//! (conv-44 cross-validation, 2026-06-13). This pass synthesizes the *set* as
//! a single high-ranking candidate at consolidation time: given all of an
//! entity's outgoing structural edges, an LLM buckets the objects into typed
//! attribute-sets and we emit one set-memory per attribute
//! ("Audrey's pets: Pepper, Precious, Panda").
//!
//! ## Why a new pass (not the topic-Infomap synthesis engine)
//! `synthesis::engine` clusters by embedding-graph Infomap (topic similarity)
//! and its prompt demands a *new abstraction*, explicitly forbidding
//! enumeration. A set-memory is the opposite: exhaustive enumeration of ONE
//! attribute of ONE entity. The clustering key here is the subject entity, and
//! the prompt forces enumeration + noise-discard.
//!
//! ## Why an LLM, not SQL `GROUP BY`
//! The LoCoMo graph collapses fine-grained relations into `related_to`
//! (≈494 of all structural edges). The pet-set and hobby-set both live inside
//! one flat `related_to` bag interleaved with noise ("photo", "girlfriend").
//! `GROUP BY (subject, predicate)` yields one blob, not answerable sets. Only
//! an LLM can separate genuine co-members from relational/event fragments.

use crate::storage::{EntitySetCandidate, Storage};
use crate::synthesis::types::{SynthesisConfig, SynthesisLlmProvider};
use serde::Deserialize;

/// Settings for the entity-set synthesis pass.
#[derive(Debug, Clone)]
pub struct EntitySetSettings {
    /// Master switch (default: false — opt-in like all retrieval levers).
    pub enabled: bool,
    /// Minimum outgoing structural-edge degree for an entity to be a candidate.
    pub min_degree: usize,
    /// Minimum distinct objects required to form any set.
    pub min_objects: usize,
    /// Cap on objects fed to the LLM per entity (bounds prompt size).
    pub max_objects_per_entity: usize,
    /// Minimum members for an emitted set (a 1-item "set" is not a list answer).
    pub min_set_members: usize,
    /// Importance assigned to emitted set-memories.
    pub set_memory_importance: f64,
    /// Max entities processed per run (bounds LLM cost).
    pub max_entities_per_run: usize,
}

impl Default for EntitySetSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            min_degree: 6,
            min_objects: 3,
            max_objects_per_entity: 60,
            min_set_members: 2,
            set_memory_importance: 0.85,
            max_entities_per_run: 32,
        }
    }
}

/// Outcome of one synthesis run.
#[derive(Debug, Default, Clone)]
pub struct EntitySetReport {
    pub candidates_considered: usize,
    pub entities_bucketed: usize,
    pub set_memories_written: usize,
    pub set_memories_updated: usize,
    pub llm_calls: usize,
    pub errors: Vec<String>,
}

/// One attribute-set returned by the LLM bucketing call.
#[derive(Debug, Clone, Deserialize)]
struct BucketedSet {
    /// Attribute label, e.g. "pets", "hobbies", "instruments".
    label: String,
    /// Members of the set (exhaustive, deduplicated by the model).
    members: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BucketingResponse {
    sets: Vec<BucketedSet>,
}

/// Build the LLM bucketing prompt for one entity's flat object list.
///
/// The prompt forces three rules that defend against the q48-style
/// noise-leak regression: (1) only group genuine co-members of ONE attribute,
/// (2) discard relational/event/possessive fragments, (3) never invent items.
pub fn build_bucketing_prompt(candidate: &EntitySetCandidate) -> String {
    let mut p = String::with_capacity(2048);
    p.push_str(
        "You group facts about a single subject into enumerable attribute-sets.\n\
         A set is a list answer to a question like \"what are X's hobbies/pets/instruments?\".\n\n\
         RULES:\n\
         1. Only group items that are genuinely co-members of ONE attribute of the subject \
         (e.g. all pets, all hobbies, all visited places). Each set must answer a single \
         enumeration question.\n\
         2. DISCARD noise: relational fragments (\"girlfriend\", \"family\"), event/action \
         phrases (\"making memories\", \"getting a dog\"), generic abstractions (\"nature\", \
         \"adventure\"), duplicates, and the subject's own name.\n\
         3. NEVER invent members not present in the input list. Preserve the exact surface \
         form of each member.\n\
         4. Only emit a set if it has 2 or more genuine members. If nothing forms a clean \
         set, return {\"sets\": []}.\n\
         5. Use a short lowercase plural noun for each label (\"pets\", \"hobbies\", \
         \"instruments\", \"visited places\").\n\n",
    );
    p.push_str(&format!("SUBJECT: {}\n\n", candidate.entity_name));
    p.push_str("FACTS (predicate -> object):\n");
    for o in &candidate.objects {
        p.push_str(&format!("- {} -> {}\n", o.predicate, o.object));
    }
    p.push_str(
        "\nReturn ONLY JSON of this exact shape, no prose:\n\
         {\"sets\": [{\"label\": \"pets\", \"members\": [\"Pepper\", \"Precious\"]}]}\n",
    );
    p
}

/// Parse the LLM bucketing response. Tolerant of code fences and surrounding
/// prose; extracts the first balanced JSON object.
pub fn parse_bucketing_response(raw: &str) -> Result<Vec<BucketedSetOut>, String> {
    let json = extract_json_object(raw).ok_or_else(|| "no JSON object in response".to_string())?;
    let resp: BucketingResponse =
        serde_json::from_str(&json).map_err(|e| format!("JSON parse: {e}"))?;
    Ok(resp
        .sets
        .into_iter()
        .map(|s| BucketedSetOut {
            label: s.label.trim().to_lowercase(),
            members: dedup_preserve_order(s.members),
        })
        .collect())
}

/// Public, owned form of a parsed set (no borrow on the response struct).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BucketedSetOut {
    pub label: String,
    pub members: Vec<String>,
}

fn dedup_preserve_order(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let t = it.trim().to_string();
        if t.is_empty() {
            continue;
        }
        let key = t.to_lowercase();
        if seen.insert(key) {
            out.push(t);
        }
    }
    out
}

/// Extract the first balanced `{...}` JSON object from arbitrary text.
fn extract_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, ch) in s[start..].char_indices() {
        match ch {
            '"' if !esc => in_str = !in_str,
            '\\' if in_str => {
                esc = !esc;
                continue;
            }
            '{' if !in_str => depth += 1,
            '}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
        esc = false;
    }
    None
}

/// Render the canonical set-memory surface form, e.g.
/// `Audrey's pets: Pepper, Precious, Panda`.
pub fn render_set_memory(entity_name: &str, label: &str, members: &[String]) -> String {
    format!("{}'s {}: {}", entity_name, label, members.join(", "))
}

/// Deterministic id for a set-memory, keyed on `(entity_id, label)` so
/// re-running the pass UPDATEs in place rather than duplicating.
pub fn set_memory_id(entity_id: &str, label: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "entity_set".hash(&mut h);
    entity_id.hash(&mut h);
    label.hash(&mut h);
    format!("eset-{:016x}", h.finish())
}

/// Run the entity-set synthesis pass: gather candidates, LLM-bucket each, emit
/// (or update) one set-memory per qualifying attribute-set.
pub fn synthesize_entity_sets(
    storage: &mut Storage,
    namespace: Option<&str>,
    settings: &EntitySetSettings,
    provider: &dyn SynthesisLlmProvider,
    synth_config: &SynthesisConfig,
) -> Result<EntitySetReport, Box<dyn std::error::Error>> {
    let mut report = EntitySetReport::default();
    if !settings.enabled {
        return Ok(report);
    }

    let mut candidates = storage.gather_entity_set_candidates(
        namespace,
        settings.min_degree,
        settings.min_objects,
        settings.max_objects_per_entity,
    )?;
    report.candidates_considered = candidates.len();
    candidates.truncate(settings.max_entities_per_run);

    for cand in &candidates {
        let prompt = build_bucketing_prompt(cand);
        let raw = match provider.generate(&prompt, synth_config) {
            Ok(r) => r,
            Err(e) => {
                report
                    .errors
                    .push(format!("bucketing {}: {e}", cand.entity_name));
                continue;
            }
        };
        report.llm_calls += 1;

        let sets = match parse_bucketing_response(&raw) {
            Ok(s) => s,
            Err(e) => {
                report
                    .errors
                    .push(format!("parse {}: {e}", cand.entity_name));
                continue;
            }
        };
        report.entities_bucketed += 1;

        for set in sets {
            // Keep only members that actually appear in the candidate's objects
            // (defends against rule-3 violations / hallucinated members).
            let present: Vec<String> = set
                .members
                .into_iter()
                .filter(|m| {
                    let ml = m.to_lowercase();
                    cand.objects.iter().any(|o| o.object.to_lowercase() == ml)
                })
                .collect();
            if present.len() < settings.min_set_members {
                continue;
            }

            let id = set_memory_id(&cand.entity_id, &set.label);
            let content = render_set_memory(&cand.entity_name, &set.label, &present);
            let metadata = serde_json::json!({
                "is_entity_set": true,
                "entity_id": cand.entity_id,
                "entity_name": cand.entity_name,
                "attribute": set.label,
                "members": present,
            });

            let existed = storage.memory_exists(&id).unwrap_or(false);
            match storage.upsert_set_memory(
                &id,
                &content,
                settings.set_memory_importance,
                Some(&serde_json::to_string(&metadata)?),
                namespace,
            ) {
                Ok(()) => {
                    if existed {
                        report.set_memories_updated += 1;
                    } else {
                        report.set_memories_written += 1;
                    }
                }
                Err(e) => report
                    .errors
                    .push(format!("write set {}/{}: {e}", cand.entity_name, set.label)),
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{EntitySetCandidate, EntitySetObject};

    fn obj(pred: &str, o: &str) -> EntitySetObject {
        EntitySetObject {
            predicate: pred.to_string(),
            object: o.to_string(),
            source_memory_id: Some("m1".to_string()),
        }
    }

    fn audrey() -> EntitySetCandidate {
        EntitySetCandidate {
            entity_id: "ent-audrey".to_string(),
            entity_name: "Audrey".to_string(),
            objects: vec![
                obj("related_to", "Pepper"),
                obj("related_to", "Precious"),
                obj("related_to", "Panda"),
                obj("related_to", "hiking"),
                obj("related_to", "bird-watching"),
                obj("related_to", "photo"),
                obj("related_to", "girlfriend"),
            ],
        }
    }

    #[test]
    fn prompt_includes_subject_and_all_facts() {
        let p = build_bucketing_prompt(&audrey());
        assert!(p.contains("SUBJECT: Audrey"));
        assert!(p.contains("related_to -> Pepper"));
        assert!(p.contains("related_to -> girlfriend"));
        assert!(p.contains("DISCARD noise"));
        assert!(p.contains("NEVER invent"));
    }

    #[test]
    fn parse_clean_response() {
        let raw = r#"{"sets": [{"label": "Pets", "members": ["Pepper", "Precious", "Panda"]}]}"#;
        let sets = parse_bucketing_response(raw).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].label, "pets"); // lowercased
        assert_eq!(sets[0].members, vec!["Pepper", "Precious", "Panda"]);
    }

    #[test]
    fn parse_tolerates_code_fence_and_prose() {
        let raw = "Here you go:\n```json\n{\"sets\": [{\"label\": \"hobbies\", \"members\": [\"hiking\", \"bird-watching\"]}]}\n```\nDone.";
        let sets = parse_bucketing_response(raw).unwrap();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].label, "hobbies");
    }

    #[test]
    fn parse_empty_sets_ok() {
        let sets = parse_bucketing_response(r#"{"sets": []}"#).unwrap();
        assert!(sets.is_empty());
    }

    #[test]
    fn parse_dedups_members_preserving_order() {
        let raw = r#"{"sets":[{"label":"pets","members":["Pepper","pepper","Panda","Pepper"]}]}"#;
        let sets = parse_bucketing_response(raw).unwrap();
        assert_eq!(sets[0].members, vec!["Pepper", "Panda"]);
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_bucketing_response("not json at all").is_err());
    }

    #[test]
    fn render_canonical_surface_form() {
        let s = render_set_memory("Audrey", "pets", &["Pepper".into(), "Panda".into()]);
        assert_eq!(s, "Audrey's pets: Pepper, Panda");
    }

    #[test]
    fn set_memory_id_is_stable_and_keyed() {
        let a = set_memory_id("ent-audrey", "pets");
        let b = set_memory_id("ent-audrey", "pets");
        let c = set_memory_id("ent-audrey", "hobbies");
        let d = set_memory_id("ent-melanie", "pets");
        assert_eq!(a, b); // idempotent
        assert_ne!(a, c); // label discriminates
        assert_ne!(a, d); // entity discriminates
        assert!(a.starts_with("eset-"));
    }

    #[test]
    fn extract_json_handles_nested_braces() {
        let raw = r#"prefix {"sets":[{"label":"x","members":["a","b"]}]} suffix"#;
        let j = extract_json_object(raw).unwrap();
        assert!(j.starts_with('{') && j.ends_with('}'));
        assert!(serde_json::from_str::<BucketingResponse>(&j).is_ok());
    }
}
