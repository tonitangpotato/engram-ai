//! Hebbian learning — co-activation forms memory links.
//!
//! "Neurons that fire together, wire together."
//!
//! When memories are recalled together repeatedly, they form Hebbian links.
//! These links create an associative network independent of explicit entity
//! tagging — purely emergent from usage patterns.

use crate::storage::Storage;
use crate::types::{CrossLink, HebbianLink};

/// Record co-activation for a set of memory IDs.
///
/// When multiple memories are retrieved together (e.g., in a single recall),
/// each pair gets their coactivation_count incremented. When the count
/// reaches the threshold, a Hebbian link is automatically formed.
pub fn record_coactivation(
    storage: &mut Storage,
    memory_ids: &[String],
    threshold: i32,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    record_coactivation_ns(storage, memory_ids, threshold, "default")
}

/// Record co-activation for a set of memory IDs with namespace tracking.
///
/// When multiple memories are retrieved together (e.g., in a single recall),
/// each pair gets their coactivation_count incremented. When the count
/// reaches the threshold, a Hebbian link is automatically formed.
pub fn record_coactivation_ns(
    storage: &mut Storage,
    memory_ids: &[String],
    threshold: i32,
    namespace: &str,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    if memory_ids.len() < 2 {
        return Ok(vec![]);
    }

    let mut new_links = vec![];

    // Generate all pairs
    for i in 0..memory_ids.len() {
        for j in (i + 1)..memory_ids.len() {
            let id1 = &memory_ids[i];
            let id2 = &memory_ids[j];

            let formed = storage.record_coactivation_ns(id1, id2, threshold, namespace)?;
            if formed {
                new_links.push((id1.clone(), id2.clone()));
            }
        }
    }

    Ok(new_links)
}

/// Memory with namespace information for cross-namespace learning.
pub struct MemoryWithNamespace {
    pub id: String,
    pub namespace: String,
}

/// Record cross-namespace co-activation for memories with known namespaces.
///
/// This is the Phase 3 extension: when memories from DIFFERENT namespaces
/// are recalled together, form cross-namespace Hebbian links.
///
/// ACL-aware: caller should verify read access before calling.
pub fn record_cross_namespace_coactivation(
    storage: &mut Storage,
    memories: &[MemoryWithNamespace],
    threshold: i32,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    if memories.len() < 2 {
        return Ok(vec![]);
    }

    let mut new_links = vec![];

    // Generate all pairs
    for i in 0..memories.len() {
        for j in (i + 1)..memories.len() {
            let m1 = &memories[i];
            let m2 = &memories[j];

            let formed = storage.record_cross_namespace_coactivation(
                &m1.id, &m1.namespace,
                &m2.id, &m2.namespace,
                threshold,
            )?;
            
            if formed {
                new_links.push((m1.id.clone(), m2.id.clone()));
            }
        }
    }

    Ok(new_links)
}

/// Discover cross-namespace Hebbian links between two namespaces.
///
/// Returns all Hebbian links that span across the given namespaces.
pub fn discover_cross_links(
    storage: &Storage,
    namespace_a: &str,
    namespace_b: &str,
) -> Result<Vec<HebbianLink>, Box<dyn std::error::Error>> {
    Ok(storage.discover_cross_links(namespace_a, namespace_b)?)
}

/// Get all cross-namespace associations for a memory.
pub fn get_cross_namespace_associations(
    storage: &Storage,
    memory_id: &str,
) -> Result<Vec<CrossLink>, Box<dyn std::error::Error>> {
    Ok(storage.get_cross_namespace_neighbors(memory_id)?)
}
