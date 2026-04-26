//! 8-dim somatic fingerprint — the agent's integrated felt-sense at a moment.
//!
//! Index semantics are LOCKED (GUARD-7). See master DESIGN-v0.3 §3.7.

use serde::{Deserialize, Serialize};

/// 8-dim snapshot of "what it was like to be the agent at this moment."
/// Index semantics are LOCKED (GUARD-7) — see master DESIGN-v0.3 §3.7.
///
///   [0] valence           — Affect; primary hedonic axis           (-1..+1)
///   [1] arousal           — Affect; activation level                ( 0..1)
///   [2] confidence        — Affect; metacognitive                   ( 0..1)
///   [3] alignment         — Affect; drive congruence                ( 0..1)
///   [4] operational_load  — Telemetry; normalized                   ( 0..1)
///   [5] cognitive_flow    — Telemetry; flow axis                   (-1..+1)
///   [6] anomaly_arousal   — Affect; novelty reading                 ( 0..1)
///   [7] feedback_recent   — Affect; short-window success rate       ( 0..1)
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SomaticFingerprint(pub [f32; 8]);

impl SomaticFingerprint {
    /// Locked dimension count (GUARD-7).
    pub const DIM: usize = 8;

    /// All-zero fingerprint; useful as a default / sentinel.
    pub const fn zero() -> Self {
        Self([0.0; 8])
    }

    /// Construct from a fixed-size array.
    pub fn from_array(v: [f32; 8]) -> Self {
        Self(v)
    }

    /// Copy of the inner array.
    pub fn as_array(&self) -> [f32; 8] {
        self.0
    }

    /// Borrow as a slice.
    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    /// [0] valence — primary hedonic axis (-1..+1).
    pub fn valence(&self) -> f32 {
        self.0[0]
    }

    /// [1] arousal — activation level (0..1).
    pub fn arousal(&self) -> f32 {
        self.0[1]
    }

    /// [2] confidence — metacognitive (0..1).
    pub fn confidence(&self) -> f32 {
        self.0[2]
    }

    /// [3] alignment — drive congruence (0..1).
    pub fn alignment(&self) -> f32 {
        self.0[3]
    }

    /// [4] operational_load — normalized telemetry (0..1).
    pub fn operational_load(&self) -> f32 {
        self.0[4]
    }

    /// [5] cognitive_flow — flow axis (-1..+1).
    pub fn cognitive_flow(&self) -> f32 {
        self.0[5]
    }

    /// [6] anomaly_arousal — novelty reading (0..1).
    pub fn anomaly_arousal(&self) -> f32 {
        self.0[6]
    }

    /// [7] feedback_recent — short-window success rate (0..1).
    pub fn feedback_recent(&self) -> f32 {
        self.0[7]
    }

    /// Cosine similarity between two fingerprints (master DESIGN §3.7).
    /// Returns 0.0 if either vector is the zero vector (avoids NaN).
    pub fn cosine_similarity(&self, other: &Self) -> f32 {
        let mut dot = 0.0f32;
        let mut na = 0.0f32;
        let mut nb = 0.0f32;
        for i in 0..Self::DIM {
            let a = self.0[i];
            let b = other.0[i];
            dot += a * b;
            na += a * a;
            nb += b * b;
        }
        if na == 0.0 || nb == 0.0 {
            return 0.0;
        }
        dot / (na.sqrt() * nb.sqrt())
    }

    /// Little-endian byte serialization for the SQLite blob format
    /// (graph design §4.1: "blob format note — somatic_fingerprint").
    /// 8 × `f32::to_le_bytes`, concatenated → 32 bytes.
    pub fn to_le_bytes(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, v) in self.0.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Decode from the 32-byte little-endian blob format. Any other length
    /// is rejected with the verbatim invariant message specified by graph
    /// design §4.1.
    pub fn from_le_bytes(bytes: &[u8]) -> Result<Self, crate::graph::GraphError> {
        if bytes.len() != 32 {
            return Err(crate::graph::GraphError::Invariant(
                "somatic fingerprint dim mismatch",
            ));
        }
        let mut arr = [0f32; 8];
        for i in 0..8 {
            let mut buf = [0u8; 4];
            buf.copy_from_slice(&bytes[i * 4..i * 4 + 4]);
            arr[i] = f32::from_le_bytes(buf);
        }
        Ok(Self(arr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphError;

    #[test]
    fn zero_is_all_zero() {
        assert_eq!(SomaticFingerprint::zero().as_array(), [0.0; 8]);
    }

    #[test]
    fn named_accessors_match_indices() {
        let arr = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        let fp = SomaticFingerprint::from_array(arr);
        assert_eq!(fp.valence(), arr[0]);
        assert_eq!(fp.arousal(), arr[1]);
        assert_eq!(fp.confidence(), arr[2]);
        assert_eq!(fp.alignment(), arr[3]);
        assert_eq!(fp.operational_load(), arr[4]);
        assert_eq!(fp.cognitive_flow(), arr[5]);
        assert_eq!(fp.anomaly_arousal(), arr[6]);
        assert_eq!(fp.feedback_recent(), arr[7]);
    }

    #[test]
    fn roundtrip_le_bytes() {
        let fp = SomaticFingerprint::from_array([
            -0.75, 0.125, 0.5, 0.875, 0.25, -0.5, 0.0625, 0.9375,
        ]);
        let bytes = fp.to_le_bytes();
        assert_eq!(bytes.len(), 32);
        let decoded = SomaticFingerprint::from_le_bytes(&bytes).unwrap();
        assert_eq!(decoded, fp);
    }

    #[test]
    fn from_le_bytes_rejects_wrong_length() {
        let short = [0u8; 31];
        match SomaticFingerprint::from_le_bytes(&short) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "somatic fingerprint dim mismatch");
            }
            other => panic!("expected Invariant, got {:?}", other),
        }

        let long = [0u8; 33];
        match SomaticFingerprint::from_le_bytes(&long) {
            Err(GraphError::Invariant(msg)) => {
                assert_eq!(msg, "somatic fingerprint dim mismatch");
            }
            other => panic!("expected Invariant, got {:?}", other),
        }
    }

    #[test]
    fn cosine_self_is_one() {
        let fp = SomaticFingerprint::from_array([
            -0.4, 0.7, 0.2, 0.9, 0.3, -0.6, 0.1, 0.8,
        ]);
        let sim = fp.cosine_similarity(&fp);
        assert!((sim - 1.0).abs() < 1e-6, "expected ≈1.0, got {sim}");
    }

    #[test]
    fn cosine_zero_vector_is_zero() {
        let zero = SomaticFingerprint::zero();
        let nz = SomaticFingerprint::from_array([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]);
        assert_eq!(zero.cosine_similarity(&nz), 0.0);
        assert_eq!(nz.cosine_similarity(&zero), 0.0);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        let a = SomaticFingerprint::from_array([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let b = SomaticFingerprint::from_array([0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let sim = a.cosine_similarity(&b);
        assert!(sim.abs() < 1e-6, "expected ≈0, got {sim}");
    }
}
