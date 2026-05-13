//! v0.4 unified substrate types.
//!
//! See `.gid/features/v04-unified-substrate/design.md` §3.1 / §3.2.
//!
//! This module is the **substrate layer** — plain Rust mirrors of the
//! `nodes` and `edges` SQL tables, with their kind discriminators and a
//! lightweight typed-attributes view. It deliberately has **no** writers,
//! readers, or pipeline logic; those land in Phases B (dual-write),
//! C (backfill), and D (read-switch) per design §5.
//!
//! ## Why a separate `Node`/`Edge` from `graph::edge`
//!
//! The pre-existing `graph::Edge` type (v0.3 ResolutionPipeline) uses
//! `Uuid`, a closed `Predicate` enum, and an `EdgeEnd` for subject-vs-
//! literal — tightly bound to the resolution code path. v0.4's substrate
//! `Edge` is a flat row mirror keyed by string ids (matching the SQL
//! `TEXT PRIMARY KEY`), with `edge_kind` and `predicate` as free-form
//! strings whose validity is enforced by the writer layer (Phase B).
//! Keeping the two types separate avoids forcing the resolution pipeline
//! to migrate before its callers are ready, per the additive-only Phase A
//! contract (design §5.1).
//!
//! ## Typed attributes: the `NodeAttributes` / `EdgeAttributes` views
//!
//! The SQL `attributes` column is JSON, and each `node_kind` /
//! `edge_kind` has its own attribute schema (memory has `tags`/`source`,
//! entity has `entity_type`/`canonical_name`, etc.). T10 ships the
//! substrate types **with `attributes: serde_json::Value` as the
//! authoritative storage** and a per-kind typed view enum
//! (`NodeAttributes` / `EdgeAttributes`) that downcasts from / upcasts to
//! the JSON. Per-kind variants are added incrementally as Phase B
//! writers populate them — `Memory` is the first variant since T12
//! (memory dual-write) is the first writer. Other kinds get added
//! alongside their dual-writers (T13 entity, T14 hebbian, T15 topic,
//! T16 provenance). Unknown / not-yet-typed kinds round-trip as
//! `NodeAttributes::Unknown(Value)`.

pub mod types;
pub mod backfill;

pub use types::{
    Edge, EdgeAttributes, EdgeKind, Node, NodeAttributes, NodeKind,
    MemoryAttributes,
};
