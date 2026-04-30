//! Spreading-activation prototype for engram multi-hop retrieval.
//!
//! Standalone exploration tied to the discussion at
//! `.gid/features/v03-retrieval/discussion-spreading-activation.md` (§9, §11, §12).
//! Loads the v0.3 typed graph DB + the v0.2 memory DB directly via rusqlite,
//! resolves anchors from question text, runs spreading activation, and scores
//! hit@5 against LoCoMo conv-26 multi-hop questions.
//!
//! USAGE:
//!   cargo run --release --example spreading_activation_prototype -p engram-bench -- \
//!     --graph-db  .../RUN-0006-substrate/locomo-conv26-iss068.graph.db \
//!     --memory-db .../RUN-0006-substrate/locomo-conv26-iss068.db \
//!     --dataset   .../cogmembench/datasets/locomo/data/locomo10.json \
//!     --namespace locomo-conv26-iss068
//!
//! NOTE: the actual namespace baked into RUN-0006 is `locomo-conv26-iss068`
//! (not `conv26` as in some early notes). The CLI default reflects that.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use uuid::Uuid;

// =============================================================================
// Parameters (per discussion §11)
// =============================================================================

const K_MAX: usize = 10;
const EPSILON: f32 = 0.01;
const DECAY_SELF: f32 = 0.7;
const DECAY_PROPAGATE: f32 = 0.5;
const PRUNING_THRESHOLD: f32 = 0.05;
const EXTRACTION_THRESHOLD: f32 = 0.02;
const ACT_MIN: f32 = -1.0;
const ACT_MAX: f32 = 1.0;

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "do", "does", "did",
    "what", "who", "when", "where", "why", "how", "which", "that", "this",
    "these", "those", "and", "or", "but", "to", "of", "in", "on", "at", "by",
    "for", "with", "from", "as", "it", "its", "he", "she", "they", "them",
    "his", "her", "their", "i", "you", "we", "us", "my", "your", "our",
    "be", "been", "being", "have", "has", "had", "will", "would", "can",
    "could", "should", "about", "into", "than", "so", "if", "then", "any",
    "all", "some", "no", "not", "yes", "out",
];

/// Conductance lookup. Supports both the canonical PascalCase predicates from
/// the discussion doc (so we can rerun on richer graphs later) and the
/// snake_case predicates that actually exist in RUN-0006 (`related_to`,
/// `leads_to`, `uses`, `part_of`, `depends_on`, `caused_by`, `is_a`).
/// Lookup is case-insensitive.
fn predicate_conductance(label: &str, kind: &str) -> f32 {
    let l = label.to_ascii_lowercase();
    match l.as_str() {
        // Canonical from discussion doc (high)
        "marriedto" | "worksat" | "bornin" | "livesin" | "authored" | "memberof"
            | "married_to" | "works_at" | "born_in" | "lives_in" | "member_of"
            => 0.8,
        "mentions" => 0.6,
        "knows" | "met" => 0.5,
        "relatedto" | "related_to" => 0.2,
        "contradicts" => -0.5,
        "mentionedin" | "mentioned_in" => 0.1,

        // RUN-0006 snake_case canonical predicates (the ones actually in the data).
        // Tuned conservatively: leads_to, uses, part_of are strong topical
        // bridges in this dataset; is_a is a type assertion (weaker, but
        // useful for kind expansion); caused_by / depends_on are causal,
        // also strong; related_to handled above.
        "leads_to"   => 0.7,
        "uses"       => 0.6,
        "part_of"    => 0.7,
        "is_a"       => 0.5,
        "caused_by"  => 0.7,
        "depends_on" => 0.6,

        _ => {
            if kind.eq_ignore_ascii_case("proposed") {
                0.3
            } else {
                0.4
            }
        }
    }
}

// =============================================================================
// CLI
// =============================================================================

#[derive(Debug)]
struct Args {
    graph_db: PathBuf,
    memory_db: PathBuf,
    dataset: PathBuf,
    namespace: String,
    dedup_entities: bool,
}

fn parse_args() -> Result<Args> {
    let mut graph_db: Option<PathBuf> = None;
    let mut memory_db: Option<PathBuf> = None;
    let mut dataset: Option<PathBuf> = None;
    let mut namespace = "locomo-conv26-iss068".to_string();
    let mut dedup_entities = false;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--graph-db" => graph_db = Some(PathBuf::from(it.next().ok_or_else(|| anyhow!("--graph-db needs value"))?)),
            "--memory-db" | "--db" => memory_db = Some(PathBuf::from(it.next().ok_or_else(|| anyhow!("--memory-db needs value"))?)),
            "--dataset" => dataset = Some(PathBuf::from(it.next().ok_or_else(|| anyhow!("--dataset needs value"))?)),
            "--namespace" | "--ns" => namespace = it.next().ok_or_else(|| anyhow!("--namespace needs value"))?,
            "--dedup-entities" => dedup_entities = true,
            "-h" | "--help" => {
                eprintln!("spreading_activation_prototype --graph-db PATH --memory-db PATH --dataset PATH [--namespace NS] [--dedup-entities]");
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown arg: {}", other)),
        }
    }
    Ok(Args {
        graph_db: graph_db.ok_or_else(|| anyhow!("--graph-db required"))?,
        memory_db: memory_db.ok_or_else(|| anyhow!("--memory-db required"))?,
        dataset: dataset.ok_or_else(|| anyhow!("--dataset required"))?,
        namespace,
        dedup_entities,
    })
}

// =============================================================================
// Graph in-memory representation
// =============================================================================

type EntityId = Uuid;

#[derive(Debug, Clone)]
struct Entity {
    name: String,
    kind: String,
}

#[derive(Debug, Clone)]
struct Edge {
    target: EntityId,
    predicate_label: String,
    predicate_kind: String,
    confidence: f32,
    valid_from: Option<f64>,
    valid_to: Option<f64>,
    /// Memory-level provenance: which memory did this edge come from?
    /// Used for memory extraction.
    memory_id: Option<String>,
}

struct Graph {
    entities: HashMap<EntityId, Entity>,
    /// outgoing[subject_id] = list of edges (entity targets only — literal-objects skipped)
    outgoing: HashMap<EntityId, Vec<Edge>>,
    /// All (entity_id, memory_id, confidence) triples — covers BOTH the entity
    /// being subject AND object on edges with memory provenance, used for
    /// memory extraction (see §9 of discussion).
    /// Map: entity_id -> Vec<(memory_id, confidence)>
    entity_to_memories: HashMap<EntityId, Vec<(String, f32)>>,
    /// canonical_name (lowercased) -> Vec<entity_id>
    name_index: HashMap<String, Vec<EntityId>>,
}

fn blob_to_uuid(blob: &[u8]) -> Result<Uuid> {
    if blob.len() != 16 {
        return Err(anyhow!("expected 16-byte UUID blob, got {}", blob.len()));
    }
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(blob);
    Ok(Uuid::from_bytes(bytes))
}

fn load_graph(path: &PathBuf, namespace: &str, dedup_entities: bool) -> Result<Graph> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open graph db: {}", path.display()))?;

    // --- entities ---
    let mut entities: HashMap<EntityId, Entity> = HashMap::new();
    let mut name_index: HashMap<String, Vec<EntityId>> = HashMap::new();
    // Remap: raw_entity_id -> canonical_entity_id. Always present; identity by
    // default. Only populated non-trivially when --dedup-entities is set.
    let mut remap: HashMap<EntityId, EntityId> = HashMap::new();

    {
        // Collect (id, name, kind) ordered so dedup is deterministic.
        let mut raw: Vec<(EntityId, String, String)> = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT id, canonical_name, kind FROM graph_entities WHERE namespace = ?1 ORDER BY id",
        )?;
        let mut rows = stmt.query(params![namespace])?;
        while let Some(row) = rows.next()? {
            let id_blob: Vec<u8> = row.get(0)?;
            let name: String = row.get(1)?;
            let kind: String = row.get(2)?;
            let id = blob_to_uuid(&id_blob)?;
            raw.push((id, name, kind));
        }

        if dedup_entities {
            // Group by (lowercased canonical_name). First id encountered (smallest by ORDER BY id)
            // wins as the canonical id for that group.
            let mut by_name: HashMap<String, EntityId> = HashMap::new();
            let mut group_size: HashMap<EntityId, usize> = HashMap::new();
            let raw_count = raw.len();
            for (id, name, kind) in &raw {
                let key = name.to_ascii_lowercase();
                let canon = *by_name.entry(key).or_insert(*id);
                remap.insert(*id, canon);
                if *id == canon {
                    entities.insert(canon, Entity { name: name.clone(), kind: kind.clone() });
                }
                *group_size.entry(canon).or_insert(0) += 1;
            }
            for (canon, _) in &entities {
                if let Some(e) = entities.get(canon) {
                    name_index.entry(e.name.to_ascii_lowercase()).or_default().push(*canon);
                }
            }
            let canonical_count = entities.len();
            let collapsed_groups = group_size.values().filter(|&&n| n > 1).count();
            let max_group = group_size.values().copied().max().unwrap_or(0);
            println!(
                "[dedup] merged {} raw entities → {} canonical (collapsed {} groups, max group size = {})",
                raw_count, canonical_count, collapsed_groups, max_group,
            );
        } else {
            for (id, name, kind) in raw {
                remap.insert(id, id);
                entities.insert(id, Entity { name: name.clone(), kind });
                name_index.entry(name.to_ascii_lowercase()).or_default().push(id);
            }
        }
    }

    // --- aliases ---
    {
        let mut stmt = conn.prepare(
            "SELECT alias, canonical_id FROM graph_entity_aliases WHERE namespace = ?1",
        )?;
        let mut rows = stmt.query(params![namespace])?;
        while let Some(row) = rows.next()? {
            let alias: String = row.get(0)?;
            let id_blob: Vec<u8> = row.get(1)?;
            let raw_id = blob_to_uuid(&id_blob)?;
            let id = remap.get(&raw_id).copied().unwrap_or(raw_id);
            if entities.contains_key(&id) {
                let key = alias.to_ascii_lowercase();
                let v = name_index.entry(key).or_default();
                if !v.contains(&id) {
                    v.push(id);
                }
            }
        }
    }

    // --- edges ---
    let mut outgoing: HashMap<EntityId, Vec<Edge>> = HashMap::new();
    let mut entity_to_memories: HashMap<EntityId, Vec<(String, f32)>> = HashMap::new();

    // CRITICAL: graph_memory_entity_mentions is the canonical Mentions table
    // (separate from graph_edges in v0.3). It links every entity to the
    // memories where it was mentioned. Without this, Person entities like
    // Caroline are completely isolated from memories in this dataset.
    {
        let mut stmt = conn.prepare(
            "SELECT entity_id, memory_id, confidence FROM graph_memory_entity_mentions WHERE namespace = ?1",
        )?;
        let mut rows = stmt.query(params![namespace])?;
        let mut n = 0usize;
        while let Some(row) = rows.next()? {
            let id_blob: Vec<u8> = row.get(0)?;
            let memory_id: String = row.get(1)?;
            let confidence: f64 = row.get(2)?;
            let raw_id = blob_to_uuid(&id_blob)?;
            let id = remap.get(&raw_id).copied().unwrap_or(raw_id);
            entity_to_memories.entry(id).or_default()
                .push((memory_id, confidence as f32));
            n += 1;
        }
        println!("[graph] mentions-table rows loaded: {}", n);
    }

    let mut edge_count = 0usize;
    let mut edge_skipped_literal = 0usize;
    {
        let mut stmt = conn.prepare(
            "SELECT subject_id, object_entity_id, predicate_label, predicate_kind,
                    confidence, valid_from, valid_to, memory_id
             FROM graph_edges
             WHERE namespace = ?1",
        )?;
        let mut rows = stmt.query(params![namespace])?;
        while let Some(row) = rows.next()? {
            let subj_blob: Vec<u8> = row.get(0)?;
            let obj_blob: Option<Vec<u8>> = row.get(1)?;
            let predicate_label: String = row.get(2)?;
            let predicate_kind: String = row.get(3)?;
            let confidence: f64 = row.get(4)?;
            let valid_from: Option<f64> = row.get(5)?;
            let valid_to: Option<f64> = row.get(6)?;
            let memory_id: Option<String> = row.get(7)?;

            let subj_raw = blob_to_uuid(&subj_blob)?;
            let subj = remap.get(&subj_raw).copied().unwrap_or(subj_raw);
            edge_count += 1;

            // Memory provenance — record for the subject regardless of object kind.
            if let Some(m) = &memory_id {
                entity_to_memories.entry(subj).or_default()
                    .push((m.clone(), confidence as f32));
            }

            let Some(obj_blob) = obj_blob else {
                // literal object — no graph edge to traverse, skip for spreading
                edge_skipped_literal += 1;
                continue;
            };
            let obj_raw = blob_to_uuid(&obj_blob)?;
            let obj = remap.get(&obj_raw).copied().unwrap_or(obj_raw);

            // Memory provenance for the object as well.
            if let Some(m) = &memory_id {
                entity_to_memories.entry(obj).or_default()
                    .push((m.clone(), confidence as f32));
            }

            // Only spread along entity→entity edges.
            outgoing.entry(subj).or_default().push(Edge {
                target: obj,
                predicate_label: predicate_label.clone(),
                predicate_kind: predicate_kind.clone(),
                confidence: confidence as f32,
                valid_from,
                valid_to,
                memory_id: memory_id.clone(),
            });
        }
    }

    println!(
        "[graph] ns={} entities={} edges={} (literal-object skipped for spreading: {}) name_index_keys={}",
        namespace,
        entities.len(),
        edge_count,
        edge_skipped_literal,
        name_index.len(),
    );

    Ok(Graph { entities, outgoing, entity_to_memories, name_index })
}

// =============================================================================
// Memory snippets (for output)
// =============================================================================

struct MemoryStore {
    /// memory_id -> (source, content_snippet)
    by_id: HashMap<String, (String, String)>,
}

fn load_memories(path: &PathBuf, namespace: &str) -> Result<MemoryStore> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open memory db: {}", path.display()))?;
    let mut by_id: HashMap<String, (String, String)> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT id, source, content FROM memories WHERE namespace = ?1",
    )?;
    let mut rows = stmt.query(params![namespace])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let source: String = row.get(1).unwrap_or_default();
        let content: String = row.get(2).unwrap_or_default();
        let snippet: String = content.chars().take(110).collect();
        by_id.insert(id, (source, snippet));
    }
    println!("[memdb] ns={} memories={}", namespace, by_id.len());
    Ok(MemoryStore { by_id })
}

// =============================================================================
// Anchor resolution
// =============================================================================

fn extract_anchors(question: &str, graph: &Graph) -> Vec<(EntityId, f32, String)> {
    // Strip possessive 's and stray punctuation so "Caroline's" matches
    // "caroline" and "identity?" matches "identity".
    let q_lower = question
        .to_ascii_lowercase()
        .replace("'s ", " ")
        .replace("'s?", " ")
        .replace("'s.", " ")
        .replace("'s,", " ")
        .replace('?', " ")
        .replace('!', " ")
        .replace('.', " ")
        .replace(',', " ");

    // Try every name in the index as a substring match. Sort longest-first so
    // multi-word names ("Caroline's mentor") win over their substrings.
    let mut keys: Vec<&String> = graph.name_index.keys().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));

    let mut consumed: Vec<(usize, usize)> = Vec::new(); // [start, end) ranges already matched
    // Group hits by their surface form (lowercased canonical key). Multiple
    // entity rows for the same canonical name (NER fragmentation in v0.3)
    // count as ONE surface-form match for normalization purposes — they all
    // get the per-surface-form strength, not 1/N split. Otherwise heavy
    // duplication starves activation below the pruning threshold.
    let mut hits_by_surface: HashMap<String, HashSet<EntityId>> = HashMap::new();

    for key in keys {
        if key.len() < 3 { continue; }
        if STOPWORDS.contains(&key.as_str()) { continue; }
        // Match all (non-overlapping, longest-wins) occurrences in the question.
        let mut search_start = 0usize;
        while let Some(range) = find_word_bounded_from(&q_lower, key, search_start) {
            let already_inside = consumed.iter().any(|(s, e)| range.0 >= *s && range.1 <= *e);
            if !already_inside {
                consumed.push(range);
                if let Some(ids) = graph.name_index.get(key) {
                    let bucket = hits_by_surface.entry(key.clone()).or_default();
                    for id in ids {
                        bucket.insert(*id);
                    }
                }
            }
            search_start = range.1;
        }
    }

    if hits_by_surface.is_empty() {
        return Vec::new();
    }

    let n_surface = hits_by_surface.len() as f32;
    let per_surface = 1.0_f32 / n_surface;
    let mut out = Vec::new();
    for (surface, ids) in hits_by_surface {
        // Each fragmented entity row gets the FULL per-surface strength.
        // (The brain analogy: firing concept "Caroline" lights up all neurons
        // representing Caroline, regardless of NER having fragmented her.)
        for id in ids {
            out.push((id, per_surface, surface.clone()));
        }
    }
    out
}

/// Word-bounded find starting from `from`: returns first (start, end) of `needle`
/// in `hay[from..]` where boundaries are non-alphanumeric.
fn find_word_bounded_from(hay: &str, needle: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || from + n.len() > bytes.len() { return None; }
    let is_wordy = |c: u8| c.is_ascii_alphanumeric();
    let mut i = from;
    while i + n.len() <= bytes.len() {
        if &bytes[i..i + n.len()] == n {
            let left_ok = i == 0 || !is_wordy(bytes[i - 1]);
            let right_ok = i + n.len() == bytes.len() || !is_wordy(bytes[i + n.len()]);
            if left_ok && right_ok {
                return Some((i, i + n.len()));
            }
        }
        i += 1;
    }
    None
}

#[allow(dead_code)]
fn find_word_bounded(hay: &str, needle: &str) -> Option<(usize, usize)> {
    find_word_bounded_from(hay, needle, 0)
}

// =============================================================================
// Activation engine
// =============================================================================

type ActState = HashMap<EntityId, f32>;

struct StepStats {
    active_nodes: usize,
    max_act: f32,
    sum_abs: f32,
    max_delta: f32,
}

fn run_activation(
    graph: &Graph,
    anchors: &[(EntityId, f32, String)],
    query_time: f64,
) -> (ActState, Vec<StepStats>, bool, usize) {
    let mut state: ActState = HashMap::new();
    for (id, strength, _surf) in anchors {
        *state.entry(*id).or_insert(0.0) += *strength;
    }

    let mut stats: Vec<StepStats> = Vec::new();
    // Step 0 stats (post-injection, pre-iteration)
    stats.push(snapshot_stats(&state, &state));

    let mut converged = false;
    let mut steps_run = 0usize;

    for step in 0..K_MAX {
        steps_run = step + 1;
        // self-decay
        let mut new_state: ActState = state.iter()
            .map(|(k, v)| (*k, *v * DECAY_SELF))
            .collect();

        // propagate
        for (node, act) in &state {
            if act.abs() < PRUNING_THRESHOLD { continue; }
            let Some(edges) = graph.outgoing.get(node) else { continue; };
            for edge in edges {
                if !edge_valid_at(edge, query_time) { continue; }
                let g = predicate_conductance(&edge.predicate_label, &edge.predicate_kind);
                let flow = *act * DECAY_PROPAGATE * g * edge.confidence;
                if flow.abs() < 1e-6 { continue; }
                *new_state.entry(edge.target).or_insert(0.0) += flow;
            }
        }

        // clamp
        for v in new_state.values_mut() {
            if *v > ACT_MAX { *v = ACT_MAX; }
            if *v < ACT_MIN { *v = ACT_MIN; }
        }

        // delta
        let mut max_delta = 0.0f32;
        let all_keys: HashSet<&EntityId> = new_state.keys().chain(state.keys()).collect();
        for k in all_keys {
            let a = state.get(k).copied().unwrap_or(0.0);
            let b = new_state.get(k).copied().unwrap_or(0.0);
            let d = (b - a).abs();
            if d > max_delta { max_delta = d; }
        }

        let s = snapshot_stats(&new_state, &state);
        stats.push(StepStats { max_delta, ..s });

        state = new_state;

        if max_delta < EPSILON {
            converged = true;
            break;
        }
    }

    (state, stats, converged, steps_run)
}

fn snapshot_stats(new_state: &ActState, _old_state: &ActState) -> StepStats {
    let active_nodes = new_state.iter().filter(|(_, v)| v.abs() > 1e-6).count();
    let max_act = new_state.values().copied().fold(0.0f32, |acc, v| acc.max(v.abs()));
    let sum_abs = new_state.values().copied().map(|v| v.abs()).sum::<f32>();
    StepStats { active_nodes, max_act, sum_abs, max_delta: 0.0 }
}

fn edge_valid_at(edge: &Edge, query_time: f64) -> bool {
    if let Some(vf) = edge.valid_from { if query_time < vf { return false; } }
    if let Some(vt) = edge.valid_to { if query_time >= vt { return false; } }
    true
}

fn extract_memories(state: &ActState, graph: &Graph, top_k: usize) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for (entity_id, act) in state {
        if act.abs() < EXTRACTION_THRESHOLD { continue; }
        let Some(mems) = graph.entity_to_memories.get(entity_id) else { continue; };
        for (mid, conf) in mems {
            *scores.entry(mid.clone()).or_insert(0.0) += *act * *conf;
        }
    }
    let mut v: Vec<(String, f32)> = scores.into_iter().collect();
    v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    v.truncate(top_k);
    v
}

// =============================================================================
// LoCoMo loader
// =============================================================================

#[derive(Debug, Clone)]
struct LocomoQA {
    /// 1-based label per discussion ("q4", "q5", "q8") = position among multi-hop.
    label: String,
    question: String,
    evidence: Vec<String>,
    answer: String,
}

fn load_locomo_multihop(path: &PathBuf, sample_id: &str, max_session: u32) -> Result<Vec<LocomoQA>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read dataset: {}", path.display()))?;
    let data: Value = serde_json::from_str(&raw)?;
    let conv = data.as_array().ok_or_else(|| anyhow!("dataset not array"))?
        .iter()
        .find(|c| c["sample_id"].as_str() == Some(sample_id))
        .ok_or_else(|| anyhow!("sample_id {} not found", sample_id))?;

    let mut out: Vec<LocomoQA> = Vec::new();
    let qa = conv["qa"].as_array().ok_or_else(|| anyhow!("qa missing"))?;
    let mut mh_idx: u32 = 0;
    for q in qa {
        if q["category"].as_u64() != Some(1) { continue; }
        let evidence: Vec<String> = q["evidence"].as_array()
            .map(|arr| arr.iter().filter_map(|e| e.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if evidence.is_empty() { continue; }
        // Skip if any evidence is from a session > max_session.
        let in_window = evidence.iter().all(|e| evidence_session(e).map(|s| s <= max_session).unwrap_or(false));
        if !in_window { continue; }
        mh_idx += 1;
        out.push(LocomoQA {
            label: format!("q{}", mh_idx),
            question: q["question"].as_str().unwrap_or("").to_string(),
            evidence,
            answer: q["answer"].as_str().map(String::from).unwrap_or_else(|| q["answer"].to_string()),
        });
    }
    Ok(out)
}

fn evidence_session(ev: &str) -> Option<u32> {
    // "D2:5" -> 2
    let rest = ev.strip_prefix('D')?;
    let (sess, _) = rest.split_once(':')?;
    sess.parse().ok()
}

// =============================================================================
// Main
// =============================================================================

fn main() -> Result<()> {
    let args = parse_args()?;
    println!("=== Spreading-Activation Prototype ===");
    println!("  graph-db:  {}", args.graph_db.display());
    println!("  memory-db: {}", args.memory_db.display());
    println!("  dataset:   {}", args.dataset.display());
    println!("  namespace: {}", args.namespace);
    println!();

    let graph = load_graph(&args.graph_db, &args.namespace, args.dedup_entities)?;
    let memstore = load_memories(&args.memory_db, &args.namespace)?;

    // Dump distinct predicate labels actually present (so unknowns are visible)
    {
        let mut seen: HashMap<(String, String), usize> = HashMap::new();
        for edges in graph.outgoing.values() {
            for e in edges {
                *seen.entry((e.predicate_label.clone(), e.predicate_kind.clone())).or_insert(0) += 1;
            }
        }
        let mut v: Vec<_> = seen.iter().collect();
        v.sort_by_key(|((l, _), _)| l.clone());
        println!("[graph] distinct predicates in spreading set:");
        for ((label, kind), n) in v {
            let g = predicate_conductance(label, kind);
            println!("  {:<14} kind={:<10} count={:<4} conductance={:.2}", label, kind, n, g);
        }
        println!();
    }

    // Load LoCoMo multi-hop QAs whose evidence is within sessions 1..=3
    // (matches RUN-0006 ingestion scope).
    let all_mh = load_locomo_multihop(&args.dataset, "conv-26", 3)?;
    println!("[dataset] in-window multi-hop QAs: {}", all_mh.len());
    // Take the first 3 — those correspond to q4, q5, q8 of the original
    // (1-indexed) multi-hop set per RUN-0006 notes.
    let target: Vec<&LocomoQA> = all_mh.iter().take(3).collect();
    println!("[dataset] running on first 3 in-window multi-hop questions:");
    for q in &target {
        println!("  {}: {:?}  evidence={:?}", q.label, q.question, q.evidence);
    }
    println!();

    // Use a query_time large enough that all valid_from values pass
    // (timestamps in the data are unix seconds; "now" is fine).
    let query_time = chrono::Utc::now().timestamp() as f64;

    let mut hits = 0usize;
    let mut total_ms: u128 = 0;
    let top_k = 5;

    for q in &target {
        println!("=== Question {}: {:?} ===", q.label, q.question);
        let t0 = Instant::now();

        let anchors = extract_anchors(&q.question, &graph);
        if anchors.is_empty() {
            println!("  ⚠ ZERO ANCHORS — counted as miss.");
            println!("  question words: {:?}", q.question.split_whitespace().collect::<Vec<_>>());
            println!("  Gold evidence: {:?}", q.evidence);
            println!("  Hit@5: NO");
            println!();
            continue;
        }

        // Group anchors by surface form for compact display.
        let mut by_surface: HashMap<String, Vec<(EntityId, f32)>> = HashMap::new();
        for (id, s, surf) in &anchors {
            by_surface.entry(surf.clone()).or_default().push((*id, *s));
        }
        println!("Anchors injected (total = {} entity instances across {} surface forms):",
                 anchors.len(), by_surface.len());
        for (surf, items) in &by_surface {
            let strength = items.iter().map(|(_, s)| *s).sum::<f32>();
            println!("  - {:<30} matched {} entities, total strength={:.3}", surf, items.len(), strength);
        }

        // Run spreading
        let (final_state, stats, converged, steps_run) =
            run_activation(&graph, &anchors, query_time);

        for (i, s) in stats.iter().enumerate() {
            if i == 0 {
                println!("Step {}: {} active, max_act={:.3}, sum|act|={:.3} (post-injection)",
                         i, s.active_nodes, s.max_act, s.sum_abs);
            } else {
                println!("Step {}: {} active, max_act={:.3}, sum|act|={:.3}, max_delta={:.4}",
                         i, s.active_nodes, s.max_act, s.sum_abs, s.max_delta);
            }
        }
        if converged {
            println!("Converged at step {} (delta < {}).", steps_run, EPSILON);
        } else {
            println!("Did not converge in {} steps; using final state.", K_MAX);
        }

        // Top entities
        let mut ents: Vec<(EntityId, f32)> = final_state.iter()
            .map(|(k, v)| (*k, *v))
            .collect();
        ents.sort_by(|a, b| b.1.abs().partial_cmp(&a.1.abs()).unwrap_or(std::cmp::Ordering::Equal));
        println!("Top 5 entities by |activation|:");
        for (id, act) in ents.iter().take(5) {
            let e = graph.entities.get(id);
            let name = e.map(|e| e.name.as_str()).unwrap_or("?");
            let kind = e.map(|e| e.kind.as_str()).unwrap_or("?");
            println!("  - {:<30} act={:+.3} kind={}", name, act, kind);
        }

        // Top memories
        let top_mems = extract_memories(&final_state, &graph, top_k);
        println!("Top {} memory candidates:", top_k);
        let evidence_set: HashSet<&String> = q.evidence.iter().collect();
        let mut hit = false;
        let mut hit_via: Option<(String, String)> = None;
        for (mid, score) in &top_mems {
            let (source, snippet) = memstore.by_id.get(mid)
                .map(|(s, sn)| (s.clone(), sn.clone()))
                .unwrap_or_else(|| ("?".into(), "(memory missing)".into()));
            let dia_id = source.rsplit('/').next().unwrap_or("").to_string();
            let mark = if evidence_set.contains(&dia_id) { "✓" } else { " " };
            println!("  {} {} score={:.3} src={} snippet={:?}",
                     mark, mid, score, dia_id, snippet);
            if evidence_set.contains(&dia_id) && !hit {
                hit = true;
                hit_via = Some((dia_id.clone(), mid.clone()));
            }
        }

        let elapsed = t0.elapsed();
        total_ms += elapsed.as_millis();

        println!("Gold evidence: {:?}", q.evidence);
        if hit {
            let (dia, mid) = hit_via.unwrap();
            println!("Hit@5: YES ({} matched memory {})", dia, mid);
            hits += 1;
        } else {
            println!("Hit@5: NO");
        }
        println!("Query time: {} ms", elapsed.as_millis());
        println!();
    }

    println!("=== Overall ===");
    let n = target.len() as f64;
    println!("hit@5: {}/{} ({:.1}%)", hits, target.len(), 100.0 * hits as f64 / n.max(1.0));
    if !target.is_empty() {
        println!("Avg query time: {} ms", total_ms / target.len() as u128);
    }

    Ok(())
}
