//! v0.2 → v0.3 type adapters used by the resolution pipeline.
//!
//! These are pure mappers — no IO, no LLM, no panics. The mapping tables are
//! the canonical truth for cross-version semantics; if a v0.2 type gains a
//! new variant, the corresponding `match` here must be extended in the same
//! commit (this is why we don't use `_ =>` fallbacks for known sources —
//! exhaustive matches force compile-time updates).
//!
//! See `.gid/features/v03-resolution/design.md` §3.2 (entity adapter) and
//! §3.3.1–3.3.2 (predicate adapter) for the mapping rationale and subtype
//! loss documentation.

use chrono::{DateTime, Utc};

use crate::entities::{EntityType as V02EntityType, ExtractedEntity};
use crate::graph::{CanonicalPredicate, EntityKind, Predicate, SomaticFingerprint};
use crate::triple::Predicate as V02Predicate;

use super::context::DraftEntity;

/// Map a v0.2 `EntityType` to a v0.3 `EntityKind`.
///
/// **Total mapping** — every v0.2 variant produces exactly one v0.3 kind.
///
/// **Subtype loss.** Some v0.2 variants (`Project`, `Technology`, `File`,
/// `Url`) collapse into broader v0.3 kinds. The lossy cases return both the
/// kind and a `subtype_hint` string that callers can preserve in
/// `Entity.summary` or alias rows when subtype precision matters.
///
/// | v0.2                  | v0.3              | Hint        |
/// |-----------------------|-------------------|-------------|
/// | `Person`              | `Person`          | none        |
/// | `Organization`        | `Organization`    | none        |
/// | `Project`             | `Organization`    | `"project"` |
/// | `Technology`          | `Artifact`        | `"technology"` |
/// | `Concept`             | `Concept`         | none        |
/// | `File`                | `Artifact`        | `"file"`    |
/// | `Url`                 | `Artifact`        | `"url"`     |
/// | `Other(s)`            | `Other(normalized(s))` | none |
pub fn map_entity_kind(v02: &V02EntityType) -> (EntityKind, Option<String>) {
    match v02 {
        V02EntityType::Person => (EntityKind::Person, None),
        V02EntityType::Organization => (EntityKind::Organization, None),
        V02EntityType::Project => (EntityKind::Organization, Some("project".into())),
        V02EntityType::Technology => (EntityKind::Artifact, Some("technology".into())),
        V02EntityType::Concept => (EntityKind::Concept, None),
        V02EntityType::File => (EntityKind::Artifact, Some("file".into())),
        V02EntityType::Url => (EntityKind::Artifact, Some("url".into())),
        V02EntityType::Other(s) => (EntityKind::other(s), None),
    }
}

/// Map a v0.2 `Predicate` to a v0.3 `CanonicalPredicate`.
///
/// **Total + lossless mapping.** All nine v0.2 variants have a 1:1
/// correspondence in v0.3 (v0.3's enum is a strict superset).
///
/// | v0.2          | v0.3 (Canonical) |
/// |---------------|------------------|
/// | `IsA`         | `IsA`            |
/// | `PartOf`      | `PartOf`         |
/// | `Uses`        | `Uses`           |
/// | `DependsOn`   | `DependsOn`      |
/// | `CausedBy`    | `CausedBy`       |
/// | `LeadsTo`     | `LeadsTo`        |
/// | `Implements`  | `Implements`     |
/// | `Contradicts` | `Contradicts`    |
/// | `RelatedTo`   | `RelatedTo`      |
pub fn map_predicate(v02: &V02Predicate) -> Predicate {
    let canon = match v02 {
        V02Predicate::IsA => CanonicalPredicate::IsA,
        V02Predicate::PartOf => CanonicalPredicate::PartOf,
        V02Predicate::Uses => CanonicalPredicate::Uses,
        V02Predicate::DependsOn => CanonicalPredicate::DependsOn,
        V02Predicate::CausedBy => CanonicalPredicate::CausedBy,
        V02Predicate::LeadsTo => CanonicalPredicate::LeadsTo,
        V02Predicate::Implements => CanonicalPredicate::Implements,
        V02Predicate::Contradicts => CanonicalPredicate::Contradicts,
        V02Predicate::RelatedTo => CanonicalPredicate::RelatedTo,
    };
    Predicate::Canonical(canon)
}

/// Normalize a free-form predicate string (from a future LLM extractor that
/// emits arbitrary labels) into a v0.3 `Predicate`.
///
/// Strategy (per §3.3.2 of resolution design):
/// 1. Try v0.2's `Predicate::from_str_lossy` — if it returns anything other
///    than `RelatedTo` (i.e. it actually matched), promote to canonical.
/// 2. **Special-case**: if the input is a recognized `RelatedTo` synonym
///    (e.g. `"related_to"`, `"associated_with"`), return canonical
///    `RelatedTo`. We detect this by checking the post-normalized form
///    against the v0.2 lossy parser's known `RelatedTo` keys.
/// 3. Otherwise (the v0.2 lossy parser would also fall back to `RelatedTo`),
///    preserve the original string verbatim as `Predicate::Proposed(label)`.
///    Schema-inducer canonicalization is deferred to v0.4 (ISS-031).
///
/// This is the **GOAL-2.13 + GOAL-1.8 / 1.9 path**: novel predicates are
/// preserved verbatim, never silently coerced into `RelatedTo`.
pub fn normalize_predicate_str(label: &str) -> Predicate {
    // Match v0.2's normalization: lowercase + replace `-` and ` ` with `_`.
    let key = label.to_lowercase().replace(['-', ' '], "_");

    // Is this an explicit RelatedTo synonym recognized by the v0.2 parser?
    let related_to_synonyms = ["related_to", "relatedto", "associated_with"];
    if related_to_synonyms.contains(&key.as_str()) {
        return Predicate::Canonical(CanonicalPredicate::RelatedTo);
    }

    // Try v0.2's lossy parser. If it returns anything other than
    // `RelatedTo`, the label *did* match a known canonical predicate.
    let lossy = V02Predicate::from_str_lossy(label);
    if !matches!(lossy, V02Predicate::RelatedTo) {
        return map_predicate(&lossy);
    }

    // The v0.2 parser would have fallen back to `RelatedTo`, but the input
    // is *not* a recognized RelatedTo synonym. Preserve the original label
    // as a Proposed predicate (do not silently coerce — GOAL-1.8/1.9).
    Predicate::Proposed(label.to_string())
}

/// Build a `DraftEntity` from a v0.2 `ExtractedEntity` mention.
///
/// The draft is **not yet resolved** — the canonical id is decided in §3.4.3
/// after candidate retrieval and signal fusion. This function only gathers
/// the per-mention information that resolution will need.
///
/// `occurred_at` is used for both `first_seen` and `last_seen` because at
/// draft time we have a single mention; resolution merges the draft with
/// existing entity history (extending `last_seen`) when the decision is
/// "merge".
pub fn draft_entity_from_mention(
    mention: &ExtractedEntity,
    occurred_at: DateTime<Utc>,
    affect_snapshot: Option<SomaticFingerprint>,
) -> DraftEntity {
    let (kind, subtype_hint) = map_entity_kind(&mention.entity_type);

    DraftEntity {
        canonical_name: mention.name.clone(),
        kind,
        aliases: vec![mention.normalized.clone()],
        subtype_hint,
        first_seen: occurred_at,
        last_seen: occurred_at,
        somatic_fingerprint: affect_snapshot,
    }
}

/// Build a `DraftEntity` for a triple endpoint (subject or object) that the
/// EntityExtractor did not produce — i.e. an entity name that appears in an
/// LLM-extracted edge but has no corresponding `ExtractedEntity` mention.
///
/// This is the "edge lift" path (ISS-048): without it, `resolve_edges` drops
/// every edge whose subject/object isn't in `ctx.entity_drafts`, even though
/// the LLM clearly identified the name as a referent.
///
/// The lifted draft is intentionally weak:
/// - `kind = EntityKind::other("unknown")` — the LLM's edge call didn't tell
///   us the type. Downstream resolution can still merge against existing
///   typed entities by name (kind is only used for tie-breaking / boosts).
/// - `aliases` carries the lowercase form, matching `draft_entity_from_mention`
///   so dedup against pattern-matched mentions works (`mention.normalized`
///   is also the lowercase form for non-Person/Url/File types).
/// - `canonical_name` keeps the raw endpoint string so `resolve_edges` can
///   match `DraftEdge::subject_name` (which is also the raw string) directly.
pub fn draft_entity_from_triple_endpoint(
    name: &str,
    occurred_at: DateTime<Utc>,
    affect_snapshot: Option<SomaticFingerprint>,
) -> DraftEntity {
    // Normalization mirrors `draft_entity_from_mention` aliases: lowercase
    // (which is what `normalize_entity_name` produces for the default case).
    // Trim removes incidental whitespace from LLM output. We do NOT lowercase
    // `canonical_name` itself — that mirrors `draft_entity_from_mention`,
    // which keeps `mention.name` raw while seeding aliases with `normalized`.
    let trimmed = name.trim();
    let alias = trimmed.to_lowercase();

    DraftEntity {
        canonical_name: trimmed.to_string(),
        kind: EntityKind::other("unknown"),
        aliases: vec![alias],
        subtype_hint: None,
        first_seen: occurred_at,
        last_seen: occurred_at,
        somatic_fingerprint: affect_snapshot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- map_entity_kind: exhaustive coverage of v0.2 EntityType ----

    #[test]
    fn map_entity_kind_person_lossless() {
        let (k, h) = map_entity_kind(&V02EntityType::Person);
        assert_eq!(k, EntityKind::Person);
        assert_eq!(h, None);
    }

    #[test]
    fn map_entity_kind_organization_lossless() {
        let (k, h) = map_entity_kind(&V02EntityType::Organization);
        assert_eq!(k, EntityKind::Organization);
        assert_eq!(h, None);
    }

    #[test]
    fn map_entity_kind_project_to_org_with_hint() {
        let (k, h) = map_entity_kind(&V02EntityType::Project);
        assert_eq!(k, EntityKind::Organization);
        assert_eq!(h.as_deref(), Some("project"));
    }

    #[test]
    fn map_entity_kind_technology_to_artifact_with_hint() {
        let (k, h) = map_entity_kind(&V02EntityType::Technology);
        assert_eq!(k, EntityKind::Artifact);
        assert_eq!(h.as_deref(), Some("technology"));
    }

    #[test]
    fn map_entity_kind_concept_lossless() {
        let (k, h) = map_entity_kind(&V02EntityType::Concept);
        assert_eq!(k, EntityKind::Concept);
        assert_eq!(h, None);
    }

    #[test]
    fn map_entity_kind_file_to_artifact_with_hint() {
        let (k, h) = map_entity_kind(&V02EntityType::File);
        assert_eq!(k, EntityKind::Artifact);
        assert_eq!(h.as_deref(), Some("file"));
    }

    #[test]
    fn map_entity_kind_url_to_artifact_with_hint() {
        let (k, h) = map_entity_kind(&V02EntityType::Url);
        assert_eq!(k, EntityKind::Artifact);
        assert_eq!(h.as_deref(), Some("url"));
    }

    #[test]
    fn map_entity_kind_other_uses_normalized_constructor() {
        // Direct `Other("Robot")` should normalize to lowercase via
        // EntityKind::other().
        let (k, h) = map_entity_kind(&V02EntityType::Other("Robot".into()));
        assert_eq!(k, EntityKind::other("Robot"));
        assert_eq!(h, None);
        // And the normalization should be observable: "Robot" and " robot "
        // produce equal kinds.
        let (k2, _) = map_entity_kind(&V02EntityType::Other(" robot ".into()));
        assert_eq!(k, k2);
    }

    // ---- map_predicate: full enum coverage of v0.2 Predicate ----

    #[test]
    fn map_predicate_full_coverage() {
        let cases = [
            (V02Predicate::IsA, CanonicalPredicate::IsA),
            (V02Predicate::PartOf, CanonicalPredicate::PartOf),
            (V02Predicate::Uses, CanonicalPredicate::Uses),
            (V02Predicate::DependsOn, CanonicalPredicate::DependsOn),
            (V02Predicate::CausedBy, CanonicalPredicate::CausedBy),
            (V02Predicate::LeadsTo, CanonicalPredicate::LeadsTo),
            (V02Predicate::Implements, CanonicalPredicate::Implements),
            (V02Predicate::Contradicts, CanonicalPredicate::Contradicts),
            (V02Predicate::RelatedTo, CanonicalPredicate::RelatedTo),
        ];
        for (v02, expected_canon) in cases {
            assert_eq!(
                map_predicate(&v02),
                Predicate::Canonical(expected_canon.clone()),
                "v0.2 {:?} should map to canonical {:?}",
                v02,
                expected_canon,
            );
        }
    }

    // ---- normalize_predicate_str: novel preservation + canonical match ----

    #[test]
    fn normalize_predicate_str_recognizes_canonical() {
        // Direct matches and common synonyms.
        let inputs = [
            ("is_a", CanonicalPredicate::IsA),
            ("isa", CanonicalPredicate::IsA),
            ("type_of", CanonicalPredicate::IsA),
            ("part_of", CanonicalPredicate::PartOf),
            ("belongs_to", CanonicalPredicate::PartOf),
            ("uses", CanonicalPredicate::Uses),
            ("utilizes", CanonicalPredicate::Uses),
            ("depends_on", CanonicalPredicate::DependsOn),
            ("requires", CanonicalPredicate::DependsOn),
            ("caused_by", CanonicalPredicate::CausedBy),
            ("due_to", CanonicalPredicate::CausedBy),
            ("leads_to", CanonicalPredicate::LeadsTo),
            ("results_in", CanonicalPredicate::LeadsTo),
            ("implements", CanonicalPredicate::Implements),
            ("realizes", CanonicalPredicate::Implements),
            ("contradicts", CanonicalPredicate::Contradicts),
            ("conflicts_with", CanonicalPredicate::Contradicts),
            ("related_to", CanonicalPredicate::RelatedTo),
            ("associated_with", CanonicalPredicate::RelatedTo),
        ];
        for (input, expected) in inputs {
            assert_eq!(
                normalize_predicate_str(input),
                Predicate::Canonical(expected.clone()),
                "input {:?} should canonicalize to {:?}",
                input,
                expected,
            );
        }
    }

    #[test]
    fn normalize_predicate_str_handles_case_and_separators() {
        // Mixed case, hyphens, and spaces all normalize.
        assert_eq!(
            normalize_predicate_str("Is-A"),
            Predicate::Canonical(CanonicalPredicate::IsA),
        );
        assert_eq!(
            normalize_predicate_str("Depends On"),
            Predicate::Canonical(CanonicalPredicate::DependsOn),
        );
        assert_eq!(
            normalize_predicate_str("PART of"),
            Predicate::Canonical(CanonicalPredicate::PartOf),
        );
    }

    #[test]
    fn normalize_predicate_str_preserves_novel_verbatim() {
        // Unknown labels become Proposed, *with the original string* (not
        // normalized — preserves the LLM's spelling for schema-inducer).
        let novel = "advisedBy";
        match normalize_predicate_str(novel) {
            Predicate::Proposed(label) => assert_eq!(label, "advisedBy"),
            other => panic!("expected Proposed, got {:?}", other),
        }

        // A novel label that happens to share a prefix with a canonical one
        // but isn't a real synonym must still be preserved as Proposed.
        let novel2 = "dependsHeavilyOn";
        match normalize_predicate_str(novel2) {
            Predicate::Proposed(label) => assert_eq!(label, "dependsHeavilyOn"),
            other => panic!("expected Proposed, got {:?}", other),
        }
    }

    #[test]
    fn normalize_predicate_str_does_not_silently_coerce_to_relatedto() {
        // The v0.2 parser would have returned RelatedTo for "foo_bar" via
        // the catch-all fallback. Our normalizer must *not* do that — it
        // must preserve the verbatim string as Proposed.
        match normalize_predicate_str("foo_bar") {
            Predicate::Proposed(label) => assert_eq!(label, "foo_bar"),
            Predicate::Canonical(c) => panic!(
                "novel label was silently coerced to canonical {:?} — \
                 this violates GOAL-1.8 / 1.9 (novel preservation)",
                c,
            ),
        }
    }

    // ---- draft_entity_from_mention ----

    #[test]
    fn draft_entity_from_mention_carries_seed_alias() {
        let mention = ExtractedEntity {
            name: "Potato".into(),
            normalized: "potato".into(),
            entity_type: V02EntityType::Person,
        };
        let now = Utc::now();
        let draft = draft_entity_from_mention(&mention, now, None);

        assert_eq!(draft.canonical_name, "Potato");
        assert_eq!(draft.kind, EntityKind::Person);
        assert_eq!(draft.aliases, vec!["potato".to_string()]);
        assert_eq!(draft.subtype_hint, None);
        assert_eq!(draft.first_seen, now);
        assert_eq!(draft.last_seen, now);
        assert!(draft.somatic_fingerprint.is_none());
    }

    #[test]
    fn draft_entity_from_mention_records_subtype_loss() {
        let mention = ExtractedEntity {
            name: "Rust".into(),
            normalized: "rust".into(),
            entity_type: V02EntityType::Technology,
        };
        let draft = draft_entity_from_mention(&mention, Utc::now(), None);

        assert_eq!(draft.kind, EntityKind::Artifact);
        assert_eq!(draft.subtype_hint.as_deref(), Some("technology"));
    }

    #[test]
    fn draft_entity_from_mention_carries_affect_snapshot() {
        let fp = SomaticFingerprint([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]);
        let mention = ExtractedEntity {
            name: "x".into(),
            normalized: "x".into(),
            entity_type: V02EntityType::Concept,
        };
        let draft = draft_entity_from_mention(&mention, Utc::now(), Some(fp));

        assert!(draft.somatic_fingerprint.is_some());
        assert_eq!(draft.somatic_fingerprint.unwrap().0[0], 0.1);
    }

    // ---- draft_entity_from_triple_endpoint (ISS-048 edge lift) ----

    #[test]
    fn draft_entity_from_triple_endpoint_keeps_raw_canonical_name() {
        // canonical_name must match what `DraftEdge::subject_name` carries
        // (raw triple string), so `resolve_edges` can do exact-string lookup.
        let now = Utc::now();
        let draft = draft_entity_from_triple_endpoint("Caroline Martinez", now, None);
        assert_eq!(draft.canonical_name, "Caroline Martinez");
    }

    #[test]
    fn draft_entity_from_triple_endpoint_alias_is_lowercased() {
        // Alias is the dedup key against pattern-matched mentions, whose
        // aliases are `mention.normalized` (lowercase for default types).
        let now = Utc::now();
        let draft = draft_entity_from_triple_endpoint("Caroline Martinez", now, None);
        assert_eq!(draft.aliases, vec!["caroline martinez".to_string()]);
    }

    #[test]
    fn draft_entity_from_triple_endpoint_trims_whitespace() {
        // LLM edge outputs can have stray whitespace; trim before storing.
        let now = Utc::now();
        let draft = draft_entity_from_triple_endpoint("  Acme  ", now, None);
        assert_eq!(draft.canonical_name, "Acme");
        assert_eq!(draft.aliases, vec!["acme".to_string()]);
    }

    #[test]
    fn draft_entity_from_triple_endpoint_kind_is_unknown_other() {
        // The LLM's edge call didn't classify the entity; we default to
        // `Other("unknown")` (NFKC+lowercase via the sanctioned constructor).
        let now = Utc::now();
        let draft = draft_entity_from_triple_endpoint("Anything", now, None);
        assert_eq!(draft.kind, EntityKind::Other("unknown".into()));
        assert!(draft.subtype_hint.is_none());
    }

    #[test]
    fn draft_entity_from_triple_endpoint_first_seen_eq_last_seen() {
        // Single-mention semantics: first_seen == last_seen at draft time.
        let now = Utc::now();
        let draft = draft_entity_from_triple_endpoint("Foo", now, None);
        assert_eq!(draft.first_seen, now);
        assert_eq!(draft.last_seen, now);
    }

    #[test]
    fn draft_entity_from_triple_endpoint_carries_affect_snapshot() {
        let fp = SomaticFingerprint([0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let now = Utc::now();
        let draft = draft_entity_from_triple_endpoint("Foo", now, Some(fp));
        assert!(draft.somatic_fingerprint.is_some());
        assert_eq!(draft.somatic_fingerprint.unwrap().0[0], 0.9);
    }

    #[test]
    fn draft_entity_from_triple_endpoint_alias_matches_mention_normalized_for_dedup() {
        // CRITICAL: lifted alias must equal `mention.normalized` for the
        // same name, so pattern-matched and edge-lifted entities dedup.
        // For a non-Person/Url/File mention, `normalize_entity_name` returns
        // simple lowercase — which is what we produce here.
        let now = Utc::now();
        let mention = ExtractedEntity {
            name: "Acme Corp".into(),
            normalized: crate::entities::normalize_entity_name(
                "Acme Corp",
                &V02EntityType::Organization,
            ),
            entity_type: V02EntityType::Organization,
        };
        let mention_draft = draft_entity_from_mention(&mention, now, None);
        let lifted_draft = draft_entity_from_triple_endpoint("Acme Corp", now, None);

        // Same alias key → dedup will collapse them.
        assert_eq!(mention_draft.aliases, lifted_draft.aliases);
    }
}
