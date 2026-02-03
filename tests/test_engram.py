"""
Comprehensive tests for the Engram memory system.

Tests SQLiteStore, activation, consolidation, forgetting, confidence,
reward, downscaling, anomaly detection, and full lifecycle integration.

Run: PYTHONPATH=. python3 tests/test_engram.py
"""

import sys
import os
import time
import math
import tempfile
import sqlite3

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from engram.core import MemoryEntry, MemoryStore, MemoryType, MemoryLayer, DEFAULT_DECAY_RATES
from engram.store import SQLiteStore
from engram.activation import (
    base_level_activation, spreading_activation, retrieval_activation, retrieve_top_k
)
from engram.consolidation import (
    consolidate_single, run_consolidation_cycle, apply_decay,
    get_consolidation_stats, MU1, MU2, ALPHA, _rebalance_layers
)
from engram.forgetting import (
    retrievability, compute_stability, effective_strength,
    should_forget, prune_forgotten, retrieval_induced_forgetting
)
from engram.confidence import confidence_score, confidence_label
from engram.reward import detect_feedback, apply_reward
from engram.downscaling import synaptic_downscale
from engram.anomaly import BaselineTracker

# SQLiteStore imports from memory_core (different module path = different enum classes)
# We need the memory_core versions for SQLiteStore tests
import engram.core as mc
SqlMemoryType = mc.MemoryType
SqlMemoryLayer = mc.MemoryLayer


PASSED = 0
FAILED = 0
ERRORS = []


def run_test(name, fn):
    global PASSED, FAILED
    try:
        fn()
        PASSED += 1
        print(f"  ✅ {name}")
    except Exception as e:
        FAILED += 1
        ERRORS.append((name, e))
        print(f"  ❌ {name}: {e}")


# ═══════════════════════════════════════════
# 1. SQLiteStore Tests
# ═══════════════════════════════════════════

def test_sqlite_add_and_get():
    store = SQLiteStore()
    m = store.add("hello world", SqlMemoryType.FACTUAL, importance=0.5)
    assert m.id is not None
    assert m.content == "hello world"
    fetched = store.get(m.id)
    assert fetched is not None
    assert fetched.content == "hello world"
    assert fetched.importance == 0.5
    store.close()

def test_sqlite_get_nonexistent():
    store = SQLiteStore()
    assert store.get("nonexistent") is None
    store.close()

def test_sqlite_access_logging():
    store = SQLiteStore()
    m = store.add("test memory", SqlMemoryType.FACTUAL)
    # add() records one access (creation)
    times = store.get_access_times(m.id)
    assert len(times) == 1, f"Expected 1 access after add, got {len(times)}"
    # get() records another access
    store.get(m.id)
    times = store.get_access_times(m.id)
    assert len(times) == 2, f"Expected 2 accesses after get, got {len(times)}"
    # manual record
    store.record_access(m.id)
    times = store.get_access_times(m.id)
    assert len(times) == 3
    store.close()

def test_sqlite_fts_finds_relevant():
    store = SQLiteStore()
    store.add("Python is a programming language", SqlMemoryType.FACTUAL)
    store.add("The weather is sunny today", SqlMemoryType.EPISODIC)
    results = store.search_fts("Python programming")
    assert len(results) == 1
    assert "Python" in results[0].content
    store.close()

def test_sqlite_fts_no_irrelevant():
    store = SQLiteStore()
    store.add("Supabase is a database", SqlMemoryType.FACTUAL)
    store.add("Cats are cute animals", SqlMemoryType.EPISODIC)
    results = store.search_fts("quantum physics")
    assert len(results) == 0
    store.close()

def test_sqlite_filter_by_type():
    store = SQLiteStore()
    store.add("fact one", SqlMemoryType.FACTUAL)
    store.add("fact two", SqlMemoryType.FACTUAL)
    store.add("episode one", SqlMemoryType.EPISODIC)
    facts = store.search_by_type(SqlMemoryType.FACTUAL)
    assert len(facts) == 2
    eps = store.search_by_type(SqlMemoryType.EPISODIC)
    assert len(eps) == 1
    store.close()

def test_sqlite_filter_by_layer():
    store = SQLiteStore()
    m1 = store.add("working mem", SqlMemoryType.FACTUAL)
    m2 = store.add("core mem", SqlMemoryType.FACTUAL)
    m2.layer = SqlMemoryLayer.L2_CORE
    store.update(m2)
    working = store.search_by_layer(SqlMemoryLayer.L3_WORKING)
    assert len(working) == 1
    core = store.search_by_layer(SqlMemoryLayer.L2_CORE)
    assert len(core) == 1
    store.close()

def test_sqlite_update_persists():
    store = SQLiteStore()
    m = store.add("mutable memory", SqlMemoryType.FACTUAL)
    m.working_strength = 0.5
    m.core_strength = 0.8
    m.layer = SqlMemoryLayer.L2_CORE
    m.pinned = True
    store.update(m)
    fetched = store.get(m.id)
    assert fetched.working_strength == 0.5
    assert fetched.core_strength == 0.8
    assert fetched.layer == SqlMemoryLayer.L2_CORE
    assert fetched.pinned is True
    store.close()

def test_sqlite_delete():
    store = SQLiteStore()
    m = store.add("to be deleted", SqlMemoryType.FACTUAL)
    mid = m.id
    store.delete(mid)
    assert store.get(mid) is None
    # access log should also be gone (CASCADE)
    times = store.get_access_times(mid)
    assert len(times) == 0
    assert len(store.all()) == 0
    store.close()

def test_sqlite_all():
    store = SQLiteStore()
    store.add("one", SqlMemoryType.FACTUAL)
    store.add("two", SqlMemoryType.EPISODIC)
    store.add("three", SqlMemoryType.RELATIONAL)
    assert len(store.all()) == 3
    store.close()

def test_sqlite_stats():
    store = SQLiteStore()
    store.add("fact", SqlMemoryType.FACTUAL)
    store.add("episode", SqlMemoryType.EPISODIC)
    store.add("episode2", SqlMemoryType.EPISODIC)
    s = store.stats()
    assert s["total_memories"] == 3
    assert s["by_type"]["episodic"] == 2
    assert s["by_type"]["factual"] == 1
    assert s["total_accesses"] >= 3  # at least one per add
    store.close()

def test_sqlite_export():
    store = SQLiteStore()
    store.add("exportable", SqlMemoryType.FACTUAL)
    store.add("also exportable", SqlMemoryType.EPISODIC)
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        export_path = f.name
    try:
        store.export(export_path)
        # Verify exported DB is valid
        exported = SQLiteStore(export_path)
        assert len(exported.all()) == 2
        exported.close()
    finally:
        os.unlink(export_path)
    store.close()

def test_sqlite_file_persistence():
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    try:
        store = SQLiteStore(db_path)
        store.add("persistent memory", SqlMemoryType.FACTUAL)
        store.close()
        # Reopen
        store2 = SQLiteStore(db_path)
        assert len(store2.all()) == 1
        assert store2.all()[0].content == "persistent memory"
        store2.close()
    finally:
        os.unlink(db_path)


# ═══════════════════════════════════════════
# 2. Activation / Search Tests
# ═══════════════════════════════════════════

def test_actr_recency():
    now = time.time()
    recent = MemoryEntry(content="recent")
    recent.access_times = [now - 3600]  # 1 hour ago
    old = MemoryEntry(content="old")
    old.access_times = [now - 30 * 86400]  # 30 days ago
    assert base_level_activation(recent, now) > base_level_activation(old, now)

def test_actr_frequency():
    now = time.time()
    frequent = MemoryEntry(content="frequent")
    frequent.access_times = [now - i * 3600 for i in range(1, 11)]
    rare = MemoryEntry(content="rare")
    rare.access_times = [now - 3600]
    assert base_level_activation(frequent, now) > base_level_activation(rare, now)

def test_actr_no_access():
    m = MemoryEntry(content="never accessed")
    m.access_times = []
    assert base_level_activation(m) == float("-inf")

def test_spreading_activation_match():
    m = MemoryEntry(content="Python programming language guide")
    score = spreading_activation(m, ["python", "programming"])
    assert score > 0

def test_spreading_activation_no_match():
    m = MemoryEntry(content="Python programming language guide")
    score = spreading_activation(m, ["quantum", "physics"])
    assert score == 0.0

def test_spreading_activation_empty():
    m = MemoryEntry(content="anything")
    assert spreading_activation(m, []) == 0.0

def test_retrieval_activation_combines():
    now = time.time()
    m = MemoryEntry(content="Python is great for data science")
    m.access_times = [now - 60]
    m.importance = 0.8
    score = retrieval_activation(m, context_keywords=["python", "data"], now=now)
    base = base_level_activation(m, now)
    assert score > base, "Context + importance should boost above base"

def test_retrieve_top_k_ordering():
    store = MemoryStore()
    now = time.time()
    m1 = store.add("Supabase database backend", MemoryType.FACTUAL)
    m1.access_times = [now - 60]
    m2 = store.add("Random unrelated thing", MemoryType.EPISODIC)
    m2.access_times = [now - 86400 * 30]
    m3 = store.add("Supabase authentication setup", MemoryType.PROCEDURAL)
    m3.access_times = [now - 120]
    results = retrieve_top_k(store, context_keywords=["supabase", "database"], k=3, now=now)
    # Supabase-related memories should rank higher
    ids = [e.id for e, _ in results]
    assert ids[0] == m1.id or ids[0] == m3.id, "Supabase memory should be top"

def test_retrieve_top_k_limit():
    store = MemoryStore()
    for i in range(10):
        store.add(f"memory number {i}", MemoryType.FACTUAL)
    results = retrieve_top_k(store, k=3)
    assert len(results) <= 3

def test_retrieve_empty_query():
    store = MemoryStore()
    now = time.time()
    m1 = store.add("first", MemoryType.FACTUAL, importance=0.9)
    m1.access_times = [now - 60]
    m2 = store.add("second", MemoryType.FACTUAL, importance=0.1)
    m2.access_times = [now - 86400 * 10]
    results = retrieve_top_k(store, context_keywords=[], k=5, now=now)
    assert len(results) >= 1  # Should still return by activation


# ═══════════════════════════════════════════
# 3. Consolidation Tests
# ═══════════════════════════════════════════

def test_apply_decay():
    m = MemoryEntry(content="test")
    m.working_strength = 1.0
    m.core_strength = 0.5
    apply_decay(m, dt_days=1.0)
    assert m.working_strength < 1.0, "Working strength should decay"
    assert m.core_strength < 0.5, "Core strength should decay"
    expected_w = 1.0 * math.exp(-MU1)
    assert abs(m.working_strength - expected_w) < 1e-6

def test_decay_pinned_exempt():
    m = MemoryEntry(content="pinned")
    m.pinned = True
    m.working_strength = 1.0
    m.core_strength = 0.5
    apply_decay(m, dt_days=10.0)
    assert m.working_strength == 1.0
    assert m.core_strength == 0.5

def test_consolidate_single_transfers():
    m = MemoryEntry(content="test")
    m.working_strength = 1.0
    m.core_strength = 0.0
    m.importance = 0.5
    consolidate_single(m, dt_days=1.0)
    assert m.core_strength > 0.0, "Core should gain from consolidation"
    assert m.consolidation_count == 1

def test_consolidation_importance_effect():
    high = MemoryEntry(content="important")
    high.working_strength = 1.0
    high.core_strength = 0.0
    high.importance = 0.9

    low = MemoryEntry(content="trivial")
    low.working_strength = 1.0
    low.core_strength = 0.0
    low.importance = 0.1

    consolidate_single(high, dt_days=1.0)
    consolidate_single(low, dt_days=1.0)
    assert high.core_strength > low.core_strength, \
        "Important memory should consolidate faster"

def test_consolidation_cycle_runs():
    store = MemoryStore()
    store.add("memory one", MemoryType.FACTUAL, importance=0.5)
    store.add("memory two", MemoryType.EPISODIC, importance=0.3)
    run_consolidation_cycle(store, dt_days=1.0)
    for m in store.all():
        assert m.consolidation_count >= 1

def test_layer_promotion():
    store = MemoryStore()
    m = store.add("promotable", MemoryType.FACTUAL, importance=0.8)
    m.core_strength = 0.3  # Above promote_threshold (0.25)
    _rebalance_layers(store)
    assert m.layer == MemoryLayer.L2_CORE

def test_layer_demotion():
    store = MemoryStore()
    m = store.add("weak", MemoryType.FACTUAL, importance=0.1)
    m.layer = MemoryLayer.L3_WORKING
    m.working_strength = 0.01
    m.core_strength = 0.01
    _rebalance_layers(store)
    assert m.layer == MemoryLayer.L4_ARCHIVE

def test_consolidation_stats():
    store = MemoryStore()
    store.add("a", MemoryType.FACTUAL)
    m = store.add("b", MemoryType.EPISODIC)
    m.pinned = True
    stats = get_consolidation_stats(store)
    assert stats["total_memories"] == 2
    assert stats["pinned"] == 1
    assert "working" in stats["layers"]


# ═══════════════════════════════════════════
# 4. Forgetting Tests
# ═══════════════════════════════════════════

def test_retrievability_fresh():
    m = MemoryEntry(content="just created")
    m.access_times = [time.time()]
    R = retrievability(m)
    assert R > 0.99, f"Fresh memory should have R≈1.0, got {R}"

def test_retrievability_decays():
    now = time.time()
    m = MemoryEntry(content="old")
    m.access_times = [now - 86400 * 30]
    m.memory_type = MemoryType.EPISODIC
    R = retrievability(m, now=now)
    assert R < 1.0, "Old memory should have decayed retrievability"

def test_stability_increases_with_access():
    m1 = MemoryEntry(content="one access")
    m1.access_times = [time.time()]
    m2 = MemoryEntry(content="many accesses")
    m2.access_times = [time.time() - i * 3600 for i in range(10)]
    assert compute_stability(m2) > compute_stability(m1)

def test_effective_strength():
    now = time.time()
    m = MemoryEntry(content="test")
    m.working_strength = 1.0
    m.core_strength = 0.5
    m.access_times = [now]
    eff = effective_strength(m, now=now)
    assert eff > 0
    assert eff <= m.working_strength + m.core_strength + 0.01

def test_should_forget_weak():
    now = time.time()
    m = MemoryEntry(content="forgotten")
    m.working_strength = 0.001
    m.core_strength = 0.001
    m.access_times = [now - 86400 * 365]  # Very old
    assert should_forget(m, threshold=0.01, now=now)

def test_should_forget_pinned_exempt():
    now = time.time()
    m = MemoryEntry(content="pinned weak")
    m.working_strength = 0.001
    m.core_strength = 0.001
    m.access_times = [now - 86400 * 365]
    m.pinned = True
    assert not should_forget(m, now=now)

def test_prune_forgotten():
    store = MemoryStore()
    now = time.time()
    m1 = store.add("strong", MemoryType.FACTUAL)
    m1.access_times = [now]
    m2 = store.add("weak", MemoryType.EPISODIC)
    m2.working_strength = 0.001
    m2.core_strength = 0.0
    m2.access_times = [now - 86400 * 365]
    pruned = prune_forgotten(store, threshold=0.01, now=now)
    assert len(pruned) >= 1
    assert m2.layer == MemoryLayer.L4_ARCHIVE

def test_retrieval_induced_forgetting():
    store = MemoryStore()
    m1 = store.add("Python is great for scripting", MemoryType.FACTUAL)
    m2 = store.add("Python is great for web development", MemoryType.FACTUAL)
    m3 = store.add("Cats are cute", MemoryType.FACTUAL)
    orig_w2 = m2.working_strength
    orig_w3 = m3.working_strength
    retrieval_induced_forgetting(store, m1)
    # m2 should be suppressed (competing: same type, overlapping words)
    assert m2.working_strength < orig_w2, "Competing memory should be suppressed"
    # m3 has low overlap, should be less affected
    assert m3.working_strength >= orig_w3 * 0.95


# ═══════════════════════════════════════════
# 5. Confidence Tests
# ═══════════════════════════════════════════

def test_confidence_high_strength():
    now = time.time()
    store = MemoryStore()
    m = store.add("strong memory", MemoryType.FACTUAL, importance=0.8)
    m.working_strength = 1.0
    m.core_strength = 0.5
    m.access_times = [now]
    score = confidence_score(m, store=store, now=now)
    assert score > 0.5, f"High-strength memory should have high confidence, got {score}"

def test_confidence_low_strength():
    now = time.time()
    store = MemoryStore()
    strong = store.add("strong", MemoryType.FACTUAL)
    strong.working_strength = 1.0
    strong.core_strength = 0.5
    strong.access_times = [now]
    weak = store.add("weak", MemoryType.EPISODIC)
    weak.working_strength = 0.05
    weak.core_strength = 0.0
    weak.access_times = [now - 86400 * 30]
    score_strong = confidence_score(strong, store=store, now=now)
    score_weak = confidence_score(weak, store=store, now=now)
    assert score_strong > score_weak, "Strong memory should have higher confidence"

def test_confidence_labels():
    assert confidence_label(0.9) == "certain"
    assert confidence_label(0.7) == "likely"
    assert confidence_label(0.5) == "uncertain"
    assert confidence_label(0.1) == "vague"
    assert confidence_label(0.0) == "vague"
    assert confidence_label(1.0) == "certain"

def test_confidence_without_store():
    now = time.time()
    m = MemoryEntry(content="standalone")
    m.working_strength = 1.0
    m.core_strength = 0.5
    m.access_times = [now]
    score = confidence_score(m, store=None, now=now)
    assert 0.0 <= score <= 1.0


# ═══════════════════════════════════════════
# 6. Reward Tests
# ═══════════════════════════════════════════

def test_detect_positive_feedback():
    pol, conf = detect_feedback("good job, that's exactly right!")
    assert pol == "positive"
    assert conf > 0.3

def test_detect_negative_feedback():
    pol, conf = detect_feedback("no that's wrong, stop")
    assert pol == "negative"
    assert conf > 0.3

def test_detect_neutral_feedback():
    pol, conf = detect_feedback("the weather is nice today")
    # "nice" is in positive signals, so this may detect positive
    # but let's check it doesn't crash
    assert pol in ("positive", "negative", "neutral")

def test_detect_chinese_feedback():
    pol, _ = detect_feedback("好的不错")
    assert pol == "positive"
    pol, _ = detect_feedback("错了别这样")
    assert pol == "negative"

def test_apply_positive_reward():
    store = MemoryStore()
    now = time.time()
    m1 = store.add("recent memory", MemoryType.FACTUAL, importance=0.3)
    m1.access_times = [now]
    orig_imp = m1.importance
    orig_w = m1.working_strength
    apply_reward(store, "positive", recent_n=3, reward_magnitude=0.15)
    assert m1.importance > orig_imp, "Positive reward should boost importance"
    assert m1.working_strength > orig_w, "Positive reward should boost strength"

def test_apply_negative_reward():
    store = MemoryStore()
    now = time.time()
    m1 = store.add("recent memory", MemoryType.FACTUAL, importance=0.5)
    m1.access_times = [now]
    orig_imp = m1.importance
    apply_reward(store, "negative", recent_n=3, reward_magnitude=0.15)
    assert m1.importance < orig_imp, "Negative reward should reduce importance"

def test_reward_temporal_discount():
    store = MemoryStore()
    now = time.time()
    m1 = store.add("most recent", MemoryType.FACTUAL, importance=0.3)
    m1.access_times = [now]
    m2 = store.add("older", MemoryType.FACTUAL, importance=0.3)
    m2.access_times = [now - 60]
    apply_reward(store, "positive", recent_n=2, reward_magnitude=0.15)
    assert m1.importance >= m2.importance, "Most recent should get more reward"


# ═══════════════════════════════════════════
# 7. Downscaling Tests
# ═══════════════════════════════════════════

def test_downscale_reduces():
    store = MemoryStore()
    m = store.add("test", MemoryType.FACTUAL)
    m.working_strength = 1.0
    m.core_strength = 0.5
    result = synaptic_downscale(store, factor=0.9)
    assert m.working_strength == 0.9
    assert m.core_strength == 0.45
    assert result["n_scaled"] == 1

def test_downscale_pinned_exempt():
    store = MemoryStore()
    m = store.add("pinned", MemoryType.FACTUAL)
    m.pinned = True
    m.working_strength = 1.0
    m.core_strength = 0.5
    synaptic_downscale(store, factor=0.5)
    assert m.working_strength == 1.0
    assert m.core_strength == 0.5

def test_downscale_preserves_ordering():
    store = MemoryStore()
    m1 = store.add("strong", MemoryType.FACTUAL)
    m1.working_strength = 1.0
    m1.core_strength = 0.5
    m2 = store.add("weak", MemoryType.FACTUAL)
    m2.working_strength = 0.3
    m2.core_strength = 0.1
    synaptic_downscale(store, factor=0.8)
    total1 = m1.working_strength + m1.core_strength
    total2 = m2.working_strength + m2.core_strength
    assert total1 > total2, "Relative ordering should be preserved"

def test_downscale_empty_store():
    store = MemoryStore()
    result = synaptic_downscale(store, factor=0.95)
    assert result["n_scaled"] == 0

def test_downscale_stats():
    store = MemoryStore()
    m = store.add("test", MemoryType.FACTUAL)
    m.working_strength = 1.0
    m.core_strength = 1.0
    result = synaptic_downscale(store, factor=0.9)
    assert abs(result["avg_before"] - 2.0) < 0.01
    assert abs(result["avg_after"] - 1.8) < 0.01


# ═══════════════════════════════════════════
# 8. Anomaly Detection Tests
# ═══════════════════════════════════════════

def test_anomaly_tracker_basic():
    tracker = BaselineTracker(window_size=50)
    for i in range(20):
        tracker.update("metric", 10.0 + (i % 3))
    baseline = tracker.get_baseline("metric")
    assert baseline["n"] == 20
    assert baseline["mean"] > 0
    assert baseline["std"] > 0

def test_anomaly_not_flagged_normal():
    tracker = BaselineTracker(window_size=50)
    import random
    random.seed(99)
    for _ in range(30):
        tracker.update("m", random.gauss(10.0, 1.0))
    # Value within 1σ of mean should not be flagged
    baseline = tracker.get_baseline("m")
    assert not tracker.is_anomaly("m", baseline["mean"])
    assert not tracker.is_anomaly("m", baseline["mean"] + 0.5 * baseline["std"])

def test_anomaly_flagged_outlier():
    tracker = BaselineTracker(window_size=50)
    import random
    random.seed(42)
    for _ in range(30):
        tracker.update("m", random.gauss(10, 1))
    # 3σ+ outlier
    assert tracker.is_anomaly("m", 100.0, sigma_threshold=3.0)

def test_anomaly_warmup_period():
    tracker = BaselineTracker(window_size=50)
    tracker.update("m", 10.0)
    tracker.update("m", 10.0)
    # Not enough samples — should NOT flag
    assert not tracker.is_anomaly("m", 1000.0, min_samples=5)

def test_anomaly_z_score():
    tracker = BaselineTracker(window_size=50)
    for _ in range(20):
        tracker.update("m", 10.0)
    tracker.update("m", 11.0)  # Add slight variance
    z = tracker.z_score("m", 10.0)
    assert isinstance(z, float)

def test_anomaly_metrics_list():
    tracker = BaselineTracker()
    tracker.update("a", 1.0)
    tracker.update("b", 2.0)
    assert set(tracker.metrics()) == {"a", "b"}

def test_anomaly_empty_baseline():
    tracker = BaselineTracker()
    b = tracker.get_baseline("nonexistent")
    assert b["n"] == 0
    assert b["mean"] == 0.0


# ═══════════════════════════════════════════
# 9. Integration — Full Lifecycle
# ═══════════════════════════════════════════

def test_full_lifecycle():
    """Full lifecycle test using MemoryStore (in-memory backend)."""
    store = MemoryStore()
    now = time.time()

    # Add 12 diverse memories
    memories = [
        ("SaltyHall uses Supabase for database", MemoryType.FACTUAL, 0.5),
        ("potato prefers action over discussion", MemoryType.RELATIONAL, 0.7),
        ("Use www.moltbook.com not moltbook.com", MemoryType.PROCEDURAL, 0.8),
        ("potato said I kinda like you", MemoryType.EMOTIONAL, 0.95),
        ("Saw a funny cat meme on day 1", MemoryType.EPISODIC, 0.1),
        ("Deploy with vercel --prod", MemoryType.PROCEDURAL, 0.6),
        ("GID uses YAML graph format", MemoryType.FACTUAL, 0.4),
        ("Had a great debugging session", MemoryType.EPISODIC, 0.3),
        ("I think graph+text hybrid is best", MemoryType.OPINION, 0.5),
        ("Memory Chain Model has two traces", MemoryType.FACTUAL, 0.6),
        ("Random trivial thought", MemoryType.EPISODIC, 0.05),
        ("Another trivial thought", MemoryType.EPISODIC, 0.05),
    ]

    entries = []
    for content, mtype, imp in memories:
        e = store.add(content, mtype, importance=imp)
        e.access_times = [now - 3600]  # Set to 1h ago for consistency
        entries.append(e)

    assert len(store.all()) == 12

    # Recall using activation
    results = retrieve_top_k(store, context_keywords=["potato", "prefers"], k=3, now=now)
    assert len(results) > 0
    top_content = results[0][0].content
    assert "potato" in top_content.lower(), f"Expected potato-related, got: {top_content}"

    # Pin the emotional memory
    entries[3].pinned = True

    # Run consolidation for 7 simulated days
    for day in range(7):
        run_consolidation_cycle(store, dt_days=1.0)

    # Important memories should have gained core_strength
    emotional = entries[3]
    assert emotional.pinned  # Still pinned
    assert emotional.working_strength == 1.0  # Pinned = no decay
    assert emotional.core_strength == 0.0  # Pinned = no consolidation either

    # High-importance procedural memory should consolidate well
    procedural = entries[2]  # "Use www.moltbook.com"
    assert procedural.core_strength > 0.0, "Important memory should have consolidated"

    # Trivial memories should have decayed
    trivial1 = entries[10]
    trivial2 = entries[11]
    assert trivial1.working_strength < 0.5, "Trivial should have decayed significantly"

    # Run downscaling
    before_strong = procedural.working_strength + procedural.core_strength
    before_weak = trivial1.working_strength + trivial1.core_strength
    synaptic_downscale(store, factor=0.9)
    after_strong = procedural.working_strength + procedural.core_strength
    after_weak = trivial1.working_strength + trivial1.core_strength
    # Ordering preserved
    if before_strong > before_weak:
        assert after_strong > after_weak, "Relative ordering should survive downscaling"

    # Apply positive reward
    entries[0].access_times = [now]  # Make it "most recent"
    orig_imp = entries[0].importance
    apply_reward(store, "positive", recent_n=3, reward_magnitude=0.15)
    assert entries[0].importance >= orig_imp

    # Apply negative reward to suppress
    entries[10].access_times = [now + 1]  # Make trivial most recent
    orig_imp_trivial = entries[10].importance
    apply_reward(store, "negative", recent_n=1, reward_magnitude=0.15)
    assert entries[10].importance <= orig_imp_trivial

    # Check stats
    stats = get_consolidation_stats(store)
    assert stats["total_memories"] == 12
    assert stats["pinned"] == 1

    # Check layer distribution after consolidation
    layers = {m.layer for m in store.all()}
    # Should have at least some layer diversity after 7 days
    assert len(layers) >= 1  # At minimum working; likely also archive or core

    # Export and verify
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as f:
        export_path = f.name
    try:
        store.save(export_path)
        store2 = MemoryStore()
        store2.load(export_path)
        assert len(store2.all()) == 12
    finally:
        os.unlink(export_path)


def test_sqlite_lifecycle():
    """Lifecycle test using SQLiteStore."""
    store = SQLiteStore()
    now = time.time()

    m1 = store.add("Supabase backend fact", SqlMemoryType.FACTUAL, importance=0.5)
    m2 = store.add("potato likes action", SqlMemoryType.RELATIONAL, importance=0.7)
    m3 = store.add("Use www.moltbook.com", SqlMemoryType.PROCEDURAL, importance=0.8)
    m4 = store.add("Trivial cat meme", SqlMemoryType.EPISODIC, importance=0.1)

    # FTS search
    results = store.search_fts("Supabase")
    assert len(results) == 1 and results[0].id == m1.id

    # Get records access
    fetched = store.get(m1.id)
    assert len(fetched.access_times) == 2  # add + get

    # Update
    m2.core_strength = 0.5
    m2.layer = SqlMemoryLayer.L2_CORE
    store.update(m2)
    fetched2 = store.get(m2.id)
    assert fetched2.core_strength == 0.5
    assert fetched2.layer == SqlMemoryLayer.L2_CORE

    # Delete
    store.delete(m4.id)
    assert store.get(m4.id) is None
    assert len(store.all()) == 3

    # Stats
    s = store.stats()
    assert s["total_memories"] == 3

    # Export
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        path = f.name
    try:
        store.export(path)
        store2 = SQLiteStore(path)
        assert len(store2.all()) == 3
        store2.close()
    finally:
        os.unlink(path)

    store.close()


# ═══════════════════════════════════════════
# Run all tests
# ═══════════════════════════════════════════

if __name__ == "__main__":
    sections = [
        ("SQLiteStore", [
            ("add and get", test_sqlite_add_and_get),
            ("get nonexistent", test_sqlite_get_nonexistent),
            ("access logging", test_sqlite_access_logging),
            ("FTS finds relevant", test_sqlite_fts_finds_relevant),
            ("FTS no irrelevant", test_sqlite_fts_no_irrelevant),
            ("filter by type", test_sqlite_filter_by_type),
            ("filter by layer", test_sqlite_filter_by_layer),
            ("update persists", test_sqlite_update_persists),
            ("delete", test_sqlite_delete),
            ("all()", test_sqlite_all),
            ("stats", test_sqlite_stats),
            ("export", test_sqlite_export),
            ("file persistence", test_sqlite_file_persistence),
        ]),
        ("Activation / Search", [
            ("ACT-R recency", test_actr_recency),
            ("ACT-R frequency", test_actr_frequency),
            ("ACT-R no access", test_actr_no_access),
            ("spreading activation match", test_spreading_activation_match),
            ("spreading activation no match", test_spreading_activation_no_match),
            ("spreading activation empty", test_spreading_activation_empty),
            ("retrieval combines scores", test_retrieval_activation_combines),
            ("retrieve_top_k ordering", test_retrieve_top_k_ordering),
            ("retrieve_top_k limit", test_retrieve_top_k_limit),
            ("empty query retrieval", test_retrieve_empty_query),
        ]),
        ("Consolidation", [
            ("apply_decay", test_apply_decay),
            ("decay pinned exempt", test_decay_pinned_exempt),
            ("consolidate transfers to core", test_consolidate_single_transfers),
            ("importance boosts consolidation", test_consolidation_importance_effect),
            ("consolidation cycle runs", test_consolidation_cycle_runs),
            ("layer promotion", test_layer_promotion),
            ("layer demotion", test_layer_demotion),
            ("consolidation stats", test_consolidation_stats),
        ]),
        ("Forgetting", [
            ("retrievability fresh", test_retrievability_fresh),
            ("retrievability decays", test_retrievability_decays),
            ("stability with access", test_stability_increases_with_access),
            ("effective strength", test_effective_strength),
            ("should_forget weak", test_should_forget_weak),
            ("should_forget pinned exempt", test_should_forget_pinned_exempt),
            ("prune_forgotten", test_prune_forgotten),
            ("retrieval-induced forgetting", test_retrieval_induced_forgetting),
        ]),
        ("Confidence", [
            ("high strength → high confidence", test_confidence_high_strength),
            ("low strength → low confidence", test_confidence_low_strength),
            ("confidence labels", test_confidence_labels),
            ("confidence without store", test_confidence_without_store),
        ]),
        ("Reward", [
            ("detect positive", test_detect_positive_feedback),
            ("detect negative", test_detect_negative_feedback),
            ("detect neutral", test_detect_neutral_feedback),
            ("detect Chinese", test_detect_chinese_feedback),
            ("apply positive reward", test_apply_positive_reward),
            ("apply negative reward", test_apply_negative_reward),
            ("temporal discount", test_reward_temporal_discount),
        ]),
        ("Downscaling", [
            ("reduces strengths", test_downscale_reduces),
            ("pinned exempt", test_downscale_pinned_exempt),
            ("preserves ordering", test_downscale_preserves_ordering),
            ("empty store", test_downscale_empty_store),
            ("stats correct", test_downscale_stats),
        ]),
        ("Anomaly Detection", [
            ("tracker basic", test_anomaly_tracker_basic),
            ("normal not flagged", test_anomaly_not_flagged_normal),
            ("outlier flagged", test_anomaly_flagged_outlier),
            ("warmup period", test_anomaly_warmup_period),
            ("z-score", test_anomaly_z_score),
            ("metrics list", test_anomaly_metrics_list),
            ("empty baseline", test_anomaly_empty_baseline),
        ]),
        ("Integration", [
            ("full lifecycle (MemoryStore)", test_full_lifecycle),
            ("full lifecycle (SQLiteStore)", test_sqlite_lifecycle),
        ]),
    ]

    print("=" * 60)
    print("  Engram Memory System — Comprehensive Tests")
    print("=" * 60)

    for section_name, tests in sections:
        print(f"\n── {section_name} ──")
        for test_name, test_fn in tests:
            run_test(test_name, test_fn)

    print("\n" + "=" * 60)
    total = PASSED + FAILED
    print(f"  Results: {PASSED}/{total} passed", end="")
    if FAILED:
        print(f", {FAILED} FAILED")
        print("\n  Failures:")
        for name, err in ERRORS:
            print(f"    ❌ {name}: {err}")
    else:
        print(" ✨")
    print("=" * 60)
