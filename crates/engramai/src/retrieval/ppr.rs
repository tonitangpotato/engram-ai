//! # Personalized PageRank (ISS-221)
//!
//! Pure, deterministic personalized-PageRank power iteration over the
//! unified entity+memory graph (HippoRAG2-style structural ranking
//! signal). This module is **side-effect free**: it takes an in-memory
//! adjacency snapshot plus a set of seed node ids and returns the
//! steady-state probability mass per node.
//!
//! ## Design (ISS-221 D3/D4/D6)
//!
//! * Standard power iteration: `x' = (1-d)·p + d·(W·x)` with damping
//!   `d = 0.85`, L1 tolerance `1e-6`, max 50 iterations.
//! * The walk is **undirected** with **uniform edge weights** (phase 1;
//!   kind-weighting is a phase-2 knob).
//! * Personalization vector `p` is uniform over the seed nodes that are
//!   present in the graph.
//! * Dangling mass (isolated nodes have no neighbors) is redistributed
//!   to the personalization vector — the standard PPR convention that
//!   keeps the walk anchored on the seeds.
//! * **Determinism (AC-2)**: node ids are interned in sorted order, so
//!   identical `(edges, seeds, cfg)` inputs produce bit-identical
//!   output regardless of edge insertion order or `HashMap` iteration
//!   order.
//!
//! ## Node identity
//!
//! The unified `nodes` table keys everything by TEXT id — entity nodes
//! by hyphenated UUID string, memory nodes by their memory-id string.
//! This module therefore uses `String` as the node id type and leaves
//! UUID↔String mapping to the caller (the orchestrator wire point).

use std::collections::{BTreeSet, HashMap};

/// Tunables for [`personalized_pagerank`]. Defaults per ISS-221 D3.
#[derive(Debug, Clone)]
pub struct PprConfig {
    /// Damping factor `d` — probability of continuing the walk vs
    /// teleporting back to the seeds. Standard 0.85.
    pub damping: f64,
    /// L1 convergence tolerance between successive iterations.
    pub tol: f64,
    /// Hard cap on power iterations (graph is small — conv-26 scale is
    /// ~1.2k nodes — so 50 is generous).
    pub max_iters: usize,
}

impl Default for PprConfig {
    fn default() -> Self {
        Self {
            damping: 0.85,
            tol: 1e-6,
            max_iters: 50,
        }
    }
}

/// Immutable adjacency snapshot, interned for index-based iteration.
///
/// Built once per query from a bulk edge read (see
/// `GraphStore::load_adjacency`). Undirected: every input edge
/// `(a, b)` produces neighbor entries in both directions. Self-loops
/// and duplicate edges are deduplicated (uniform weights make
/// duplicates meaningless and self-loops only delay convergence).
#[derive(Debug, Clone)]
pub struct Adjacency {
    /// Sorted node ids; index in this Vec == node index everywhere else.
    ids: Vec<String>,
    /// id → index lookup.
    index: HashMap<String, usize>,
    /// Neighbor lists by node index. Each list is sorted ascending and
    /// deduplicated.
    neighbors: Vec<Vec<usize>>,
}

impl Adjacency {
    /// Build an adjacency snapshot from an undirected edge list.
    ///
    /// Determinism: node ids are collected into a `BTreeSet` first, so
    /// the interned ordering — and therefore all downstream float
    /// accumulation order — is independent of input edge order.
    pub fn from_edges<I>(edges: I) -> Self
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let edges: Vec<(String, String)> = edges.into_iter().collect();

        let mut id_set: BTreeSet<&str> = BTreeSet::new();
        for (a, b) in &edges {
            id_set.insert(a.as_str());
            id_set.insert(b.as_str());
        }
        let ids: Vec<String> = id_set.into_iter().map(str::to_owned).collect();
        let index: HashMap<String, usize> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();

        let mut neighbors: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); ids.len()];
        for (a, b) in &edges {
            let (ia, ib) = (index[a.as_str()], index[b.as_str()]);
            if ia == ib {
                continue; // skip self-loops
            }
            neighbors[ia].insert(ib);
            neighbors[ib].insert(ia);
        }
        let neighbors = neighbors
            .into_iter()
            .map(|set| set.into_iter().collect())
            .collect();

        Self {
            ids,
            index,
            neighbors,
        }
    }

    /// Number of nodes in the snapshot.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// True when the snapshot has no nodes.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Index of a node id, if present.
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }
}

/// Run personalized PageRank from `seeds` over `adj`.
///
/// Returns `None` when there is nothing to rank:
/// * `seeds` is empty (no resolved anchors — the plan should skip the
///   channel entirely, ISS-221 module plan), or
/// * none of the seeds exist in the graph snapshot, or
/// * the graph is empty.
///
/// Otherwise returns the steady-state probability mass for **every**
/// node in the snapshot (mass sums to ~1.0 up to float error). Score
/// extraction/normalization against a candidate pool is the caller's
/// concern (ISS-221 D5).
pub fn personalized_pagerank(
    adj: &Adjacency,
    seeds: &[String],
    cfg: &PprConfig,
) -> Option<HashMap<String, f64>> {
    if adj.is_empty() || seeds.is_empty() {
        return None;
    }

    // Resolve seeds → indices, dropping unknown ids. BTreeSet for
    // deterministic ordering and dedup (duplicate seeds must not get
    // double personalization mass).
    let seed_idx: BTreeSet<usize> = seeds
        .iter()
        .filter_map(|s| adj.index_of(s))
        .collect();
    if seed_idx.is_empty() {
        return None;
    }

    let n = adj.len();
    let d = cfg.damping;
    let seed_mass = 1.0 / seed_idx.len() as f64;

    // Personalization vector: uniform over resolved seeds.
    let mut p = vec![0.0_f64; n];
    for &i in &seed_idx {
        p[i] = seed_mass;
    }

    // Start from the personalization vector (standard choice; any
    // distribution converges, this one converges fastest for PPR).
    let mut x = p.clone();
    let mut next = vec![0.0_f64; n];

    for _ in 0..cfg.max_iters {
        // Walk step: distribute x[i]/deg(i) to each neighbor.
        // Dangling nodes (deg 0) contribute their mass to the
        // personalization vector instead.
        next.fill(0.0);
        let mut dangling_mass = 0.0_f64;
        for i in 0..n {
            let nbrs = &adj.neighbors[i];
            if nbrs.is_empty() {
                dangling_mass += x[i];
                continue;
            }
            let share = x[i] / nbrs.len() as f64;
            for &j in nbrs {
                next[j] += share;
            }
        }

        // x' = (1-d)·p + d·(walk + dangling→p)
        let mut diff = 0.0_f64;
        for i in 0..n {
            let teleport = (1.0 - d) + d * dangling_mass;
            let v = teleport * p[i] + d * next[i];
            diff += (v - x[i]).abs();
            x[i] = v;
        }

        if diff < cfg.tol {
            break;
        }
    }

    Some(
        adj.ids
            .iter()
            .cloned()
            .zip(x.iter().copied())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    /// Simple line graph a—b—c seeded on `a`. Note: at damping 0.85
    /// the degree-2 hub `b` legitimately accumulates MORE mass than
    /// the degree-1 seed (analytic steady state: a=0.3453, b=0.4595,
    /// c=0.1953) — seed-outranks-all is NOT a PPR invariant. What PPR
    /// guarantees is locality: the seed outranks equally-degreed nodes
    /// farther away (a > c).
    #[test]
    fn converges_to_analytic_steady_state() {
        let adj = Adjacency::from_edges([e("a", "b"), e("b", "c")]);
        let scores =
            personalized_pagerank(&adj, &["a".to_string()], &PprConfig::default()).unwrap();

        assert_eq!(scores.len(), 3);
        let (sa, sb, sc) = (scores["a"], scores["b"], scores["c"]);

        // Analytic solution of x = 0.15·p + 0.85·W·x for this graph:
        //   x_b = 0.1275 / 0.2775, x_a = 0.15 + 0.425·x_b, x_c = 0.425·x_b
        // Tolerance note: convergence rate is d=0.85/iter, so the
        // default max_iters=50 floor leaves ~1e-4 residual before the
        // 1e-6 L1 tol can be reached (would need ~80 iters). 1e-3 is
        // the honest bound for default config.
        let xb = 0.1275_f64 / 0.2775_f64;
        let xa = 0.15 + 0.425 * xb;
        let xc = 0.425 * xb;
        assert!((sa - xa).abs() < 1e-3, "a: got {sa}, want {xa}");
        assert!((sb - xb).abs() < 1e-3, "b: got {sb}, want {xb}");
        assert!((sc - xc).abs() < 1e-3, "c: got {sc}, want {xc}");

        // Locality invariant: seed outranks the same-degree 2-hop node.
        assert!(sa > sc, "seed must outrank 2-hop peer: {sa} vs {sc}");

        // Probability mass sums to ~1.
        let total: f64 = scores.values().sum();
        assert!((total - 1.0).abs() < 1e-6, "mass should sum to 1, got {total}");
    }

    /// A node present only as an isolated seed (no edges) is a dangling
    /// node — its mass must be redistributed via the personalization
    /// vector, and iteration must still converge with mass ≈ 1.
    #[test]
    fn dangling_nodes_redistribute_mass() {
        // "lone" participates in one edge to make it part of the
        // snapshot, but we test the dangling path with a star where the
        // hub is removed conceptually: build a graph that contains a
        // genuinely isolated node via a self-loop-only edge.
        let adj = Adjacency::from_edges([e("a", "b"), e("lone", "lone")]);
        // Self-loop is skipped → "lone" has no neighbors → dangling.
        assert!(adj.index_of("lone").is_some());

        let scores = personalized_pagerank(
            &adj,
            &["lone".to_string()],
            &PprConfig::default(),
        )
        .unwrap();

        let total: f64 = scores.values().sum();
        assert!((total - 1.0).abs() < 1e-6, "mass should sum to 1, got {total}");
        // The dangling seed keeps the bulk of the mass (teleport +
        // dangling redistribution both return to it).
        assert!(scores["lone"] > scores["a"]);
        assert!(scores["lone"] > scores["b"]);
    }

    /// Seeding in one component leaves the other component with ~zero
    /// mass — PPR localizes around the seeds.
    #[test]
    fn disconnected_component_gets_negligible_mass() {
        let adj = Adjacency::from_edges([
            e("a", "b"),
            e("b", "c"),
            // Disconnected island.
            e("x", "y"),
        ]);
        let scores =
            personalized_pagerank(&adj, &["a".to_string()], &PprConfig::default()).unwrap();

        assert!(scores["x"] < 1e-9, "island node x: {}", scores["x"]);
        assert!(scores["y"] < 1e-9, "island node y: {}", scores["y"]);
        assert!(scores["a"] > 0.1);
    }

    /// Bit-identical output for identical inputs, regardless of edge
    /// insertion order (AC-2 determinism).
    #[test]
    fn deterministic_across_edge_orderings() {
        let edges_fwd = vec![e("m1", "ent1"), e("ent1", "ent2"), e("ent2", "m2"), e("m1", "ent2")];
        let mut edges_rev = edges_fwd.clone();
        edges_rev.reverse();

        let adj1 = Adjacency::from_edges(edges_fwd);
        let adj2 = Adjacency::from_edges(edges_rev);
        let seeds = vec!["ent1".to_string(), "ent2".to_string()];
        let cfg = PprConfig::default();

        let s1 = personalized_pagerank(&adj1, &seeds, &cfg).unwrap();
        let s2 = personalized_pagerank(&adj2, &seeds, &cfg).unwrap();

        assert_eq!(s1.len(), s2.len());
        for (id, v1) in &s1 {
            let v2 = s2[id];
            assert!(
                v1.to_bits() == v2.to_bits(),
                "node {id}: {v1} vs {v2} not bit-identical"
            );
        }
    }

    /// Duplicate seeds must not double-weight the personalization mass.
    #[test]
    fn duplicate_seeds_deduplicated() {
        let adj = Adjacency::from_edges([e("a", "b"), e("b", "c")]);
        let cfg = PprConfig::default();

        let once = personalized_pagerank(&adj, &["a".to_string()], &cfg).unwrap();
        let twice = personalized_pagerank(
            &adj,
            &["a".to_string(), "a".to_string()],
            &cfg,
        )
        .unwrap();

        for (id, v1) in &once {
            assert!(v1.to_bits() == twice[id].to_bits(), "node {id} diverged");
        }
    }

    /// Empty seeds → None (plan skips the channel).
    #[test]
    fn empty_seeds_short_circuit() {
        let adj = Adjacency::from_edges([e("a", "b")]);
        assert!(personalized_pagerank(&adj, &[], &PprConfig::default()).is_none());
    }

    /// Seeds that don't exist in the snapshot → None.
    #[test]
    fn unknown_seeds_short_circuit() {
        let adj = Adjacency::from_edges([e("a", "b")]);
        let seeds = vec!["nope".to_string()];
        assert!(personalized_pagerank(&adj, &seeds, &PprConfig::default()).is_none());
    }

    /// Empty graph → None.
    #[test]
    fn empty_graph_short_circuit() {
        let adj = Adjacency::from_edges(Vec::<(String, String)>::new());
        let seeds = vec!["a".to_string()];
        assert!(personalized_pagerank(&adj, &seeds, &PprConfig::default()).is_none());
    }

    /// Mixed seed set where only some seeds resolve: unresolved ids are
    /// dropped, resolved ones still personalize the walk.
    #[test]
    fn partially_resolved_seeds_use_known_subset() {
        let adj = Adjacency::from_edges([e("a", "b"), e("b", "c")]);
        let cfg = PprConfig::default();

        let mixed = personalized_pagerank(
            &adj,
            &["a".to_string(), "ghost".to_string()],
            &cfg,
        )
        .unwrap();
        let pure = personalized_pagerank(&adj, &["a".to_string()], &cfg).unwrap();

        for (id, v) in &pure {
            assert!(v.to_bits() == mixed[id].to_bits(), "node {id} diverged");
        }
    }
}
