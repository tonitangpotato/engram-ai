//! ISS-206 verification probe — is the gold episode's resolved date
//! ACTUALLY stranded (absent from the text the generator reads), or does
//! the existing ISS-190/191 surfacing path already expose it?
//!
//! The gold episode for conv-26 q0 is "Caroline attended a LGBTQ support
//! group" (node a838a102 in forensic DB .tmpK8lZyN). Its resolved date
//! 2023-05-07 lives in attributes.engram.dimensions.temporal = {kind:day,
//! value:"2023-05-07"}, while occurred_at is the conversation timestamp
//! (2023-05-08, off by one).
//!
//! This probe loads the gold record through the EXACT production retrieval
//! loader path (Storage::get_by_ids → row_to_record_from_node_impl, which
//! parses `attributes` into MemoryRecord.metadata), then reproduces what
//! the bench generator's `derived_temporal_value` + `[when] content` line
//! would be.
//!
//!   line == "[2023-05-07] Caroline attended a LGBTQ support group"
//!       => date is NOT stranded; ISS-206 premise is already satisfied by
//!          the surfacing path; q0 failure (if any) is elsewhere.
//!   metadata == None / temporal absent
//!       => date IS stranded on the retrieval read path; ISS-206 is real.
//!
//! Run:
//!   export PATH="$HOME/.cargo/bin:$PATH"
//!   cargo run -p engramai --example iss206_date_surface_probe
//!
//! Read-only, deterministic, no LLM.

use engramai::storage::Storage;

const GOLD_ID: &str = "a838a102";

fn default_db() -> String {
    std::env::var("ISS206_DB_PATH").unwrap_or_else(|_| {
        "/var/folders/48/npr42z0967b376x1rc7wbp6m0000gn/T/.tmpK8lZyN/substrate.db".into()
    })
}

/// Mirror of engram-bench `derived_temporal_value`: reads
/// /engram/dimensions/temporal/value from MemoryRecord.metadata.
fn derived_temporal_value(meta: &Option<serde_json::Value>) -> Option<String> {
    let v = meta
        .as_ref()?
        .pointer("/engram/dimensions/temporal/value")
        .and_then(|v| v.as_str())?
        .trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

fn main() {
    let db_path = default_db();
    println!("=== ISS-206 date-surface probe ===");
    println!("DB: {db_path}\n");

    let storage = Storage::new(&db_path).expect("open storage");
    let rows = storage.get_by_ids(&[GOLD_ID]).expect("get_by_ids");
    let rec = match rows.into_iter().next() {
        Some(r) => r,
        None => {
            println!("GOLD {GOLD_ID} not found via get_by_ids (deleted/superseded?)");
            return;
        }
    };

    println!("content       : {:?}", rec.content);
    println!("occurred_at   : {:?}", rec.occurred_at);
    println!("metadata None?: {}", rec.metadata.is_none());

    let derived = derived_temporal_value(&rec.metadata);
    println!("derived_temporal_value: {derived:?}");

    // Reproduce the bench generator line (surfacing ON, the default).
    // engram-bench format_context_block: `when = derived.or(occurred_at)`.
    let when = derived.clone().unwrap_or_else(|| {
        rec.occurred_at
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "<no date>".into())
    });
    let line = format!("[{when}] {}", rec.content);
    println!("\ngenerator line (surfacing ON):\n    {line}");

    match derived.as_deref() {
        Some("2023-05-07") => println!(
            "\nVERDICT: date NOT stranded — temporal.value=2023-05-07 surfaces \
             into the generator line. ISS-206 premise already satisfied by the \
             ISS-190/191 surfacing path."
        ),
        Some(other) => println!(
            "\nVERDICT: a date surfaces ({other}) but it is not the gold \
             2023-05-07 — investigate temporal extraction."
        ),
        None => println!(
            "\nVERDICT: NO derived date — generator falls back to occurred_at. \
             If metadata is None the retrieval read path strips the temporal \
             dimension and ISS-206 is real on the read path."
        ),
    }
}
