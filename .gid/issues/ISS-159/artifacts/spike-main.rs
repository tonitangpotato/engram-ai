// Cross-encoder spike for ISS-159 weapon A (v3 — ort 2.x canonical API).
use anyhow::{anyhow, Context, Result};
use ndarray::Array2;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::TensorRef;
use std::path::PathBuf;
use std::time::Instant;
use tokenizers::Tokenizer;

fn model_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap()
        .join(".cache/engram/models/ms-marco-MiniLM-L-6-v2")
}

fn main() -> Result<()> {
    let dir = model_dir();
    let onnx = dir.join("model.onnx");
    let tok_json = dir.join("tokenizer.json");

    println!("loading tokenizer from {}", tok_json.display());
    let tokenizer = Tokenizer::from_file(&tok_json)
        .map_err(|e| anyhow!("tokenizer load: {e}"))?;

    println!("loading ONNX session from {}", onnx.display());
    let t0 = Instant::now();
    let mut session = Session::builder()
        .map_err(|e| anyhow!("Session::builder: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow!("opt_level: {e}"))?
        .with_intra_threads(4)
        .map_err(|e| anyhow!("intra_threads: {e}"))?
        .commit_from_file(&onnx)
        .with_context(|| format!("commit_from_file({})", onnx.display()))?;
    println!("  loaded in {:?}", t0.elapsed());

    println!("\ninputs:");
    for inp in session.inputs() {
        println!("  - {}", inp.name());
    }
    println!("outputs:");
    for out in session.outputs() {
        println!("  - {}", out.name());
    }

    let query = "What is the capital of France?";
    let docs = [
        ("highly relevant", "Paris is the capital and most populous city of France."),
        ("irrelevant",      "The mitochondrion is the powerhouse of the cell."),
        ("partial",         "France is a country in Western Europe with many beautiful cities."),
    ];

    println!("\nquery: {query:?}");
    for (label, doc) in docs.iter() {
        let pair_t0 = Instant::now();
        let score = score_pair(&mut session, &tokenizer, query, doc)?;
        println!(
            "  [{label:>16}] score={score:>+8.4}  ({:?})  doc={doc:?}",
            pair_t0.elapsed()
        );
    }

    println!("\nlatency probe: 50 pairs at K_fusion=50");
    let bulk_t0 = Instant::now();
    for i in 0..50 {
        let _ = score_pair(&mut session, &tokenizer, query, docs[i % docs.len()].1)?;
    }
    let total = bulk_t0.elapsed();
    println!("  total={:?}  per-pair={:?}", total, total / 50);

    Ok(())
}

fn score_pair(
    session: &mut Session,
    tokenizer: &Tokenizer,
    query: &str,
    doc: &str,
) -> Result<f32> {
    let enc = tokenizer
        .encode((query, doc), true)
        .map_err(|e| anyhow!("tokenize: {e}"))?;

    let ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
    let mask: Vec<i64> = enc.get_attention_mask().iter().map(|&x| x as i64).collect();
    let type_ids: Vec<i64> = enc.get_type_ids().iter().map(|&x| x as i64).collect();
    let seq_len = ids.len();

    let ids_arr = Array2::from_shape_vec((1, seq_len), ids)?;
    let mask_arr = Array2::from_shape_vec((1, seq_len), mask)?;
    let type_arr = Array2::from_shape_vec((1, seq_len), type_ids)?;

    let outputs = session.run(ort::inputs![
        "input_ids" => TensorRef::from_array_view(&ids_arr).map_err(|e| anyhow!("ids tensor: {e}"))?,
        "attention_mask" => TensorRef::from_array_view(&mask_arr).map_err(|e| anyhow!("mask tensor: {e}"))?,
        "token_type_ids" => TensorRef::from_array_view(&type_arr).map_err(|e| anyhow!("type tensor: {e}"))?,
    ]).map_err(|e| anyhow!("session.run: {e}"))?;

    let (shape, data) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow!("extract logits: {e}"))?;
    let total: i64 = shape.iter().product();
    if total == 0 {
        anyhow::bail!("empty logits output");
    }
    let mean = data.iter().sum::<f32>() / total as f32;
    Ok(mean)
}
