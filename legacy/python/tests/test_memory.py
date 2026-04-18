"""
Tests demonstrating the neuroscience-grounded memory system.

Each test validates a specific brain-inspired mechanism.
"""

import time
import math
from engram.core import MemoryEntry, MemoryStore, MemoryType, MemoryLayer
from engram.activation import base_level_activation, retrieval_activation, retrieve_top_k
from engram.consolidation import (
    consolidate_single, run_consolidation_cycle, apply_decay,
    get_consolidation_stats, MU1, MU2, ALPHA
)
from engram.forgetting import (
    retrievability, compute_stability, effective_strength,
    should_forget, prune_forgotten, retrieval_induced_forgetting
)


def test_actr_recency_effect():
    """
    ACT-R: More recent accesses → higher activation.

    Like the brain: "What did I do yesterday?" is easier than
    "What did I do 3 months ago?"
    """
    now = time.time()

    # Memory accessed 1 hour ago
    recent = MemoryEntry(content="recent event")
    recent.access_times = [now - 3600]

    # Memory accessed 30 days ago
    old = MemoryEntry(content="old event")
    old.access_times = [now - 30 * 86400]

    recent_activation = base_level_activation(recent, now=now)
    old_activation = base_level_activation(old, now=now)

    print(f"  Recent (1h ago):  activation = {recent_activation:.3f}")
    print(f"  Old (30d ago):    activation = {old_activation:.3f}")
    assert recent_activation > old_activation, "Recent should have higher activation"


def test_actr_frequency_effect():
    """
    ACT-R: More frequently accessed → higher activation.

    Like the brain: your own name has high activation because
    you've encountered it thousands of times.
    """
    now = time.time()

    # Memory accessed once
    single = MemoryEntry(content="accessed once")
    single.access_times = [now - 86400]

    # Memory accessed 10 times (daily for 10 days)
    frequent = MemoryEntry(content="accessed many times")
    frequent.access_times = [now - i * 86400 for i in range(1, 11)]

    single_act = base_level_activation(single, now=now)
    frequent_act = base_level_activation(frequent, now=now)

    print(f"  Single access:    activation = {single_act:.3f}")
    print(f"  10 accesses:      activation = {frequent_act:.3f}")
    assert frequent_act > single_act, "Frequent should have higher activation"


def test_actr_context_retrieval():
    """
    ACT-R spreading activation: context keywords boost relevant memories.

    Like the brain: being in a kitchen makes food-related memories
    more accessible.
    """
    store = MemoryStore()
    store.add("SaltyHall uses Supabase for database", MemoryType.FACTUAL)
    store.add("potato likes fast iteration", MemoryType.RELATIONAL)
    store.add("Use www.moltbook.com not moltbook.com", MemoryType.PROCEDURAL)
    store.add("The weather was nice today", MemoryType.EPISODIC)

    results = retrieve_top_k(store, context_keywords=["database", "supabase"], k=2)

    print("  Query: ['database', 'supabase']")
    for entry, score in results:
        print(f"    {score:.3f}: {entry.content[:50]}")

    assert "Supabase" in results[0][0].content, "Supabase memory should rank first"


def test_memory_chain_consolidation():
    """
    Memory Chain Model: Working strength decays fast, core strength grows slowly.

    Like the brain: hippocampal traces fade within days,
    but neocortical traces are built up through repeated consolidation.
    """
    entry = MemoryEntry(
        content="Important lesson learned",
        memory_type=MemoryType.PROCEDURAL,
        importance=0.8,
        working_strength=1.0,
        core_strength=0.0,
    )

    print(f"  Day 0: working={entry.working_strength:.3f}, core={entry.core_strength:.3f}")

    # Simulate 7 days of consolidation (one "sleep" per day)
    for day in range(1, 8):
        consolidate_single(entry, dt_days=1.0)
        print(f"  Day {day}: working={entry.working_strength:.3f}, core={entry.core_strength:.3f}")

    assert entry.working_strength < 0.5, "Working trace should have decayed significantly"
    assert entry.core_strength > 0.1, "Core trace should have grown from consolidation"


def test_emotional_memories_consolidate_faster():
    """
    Emotional memories consolidate faster (amygdala modulation).

    Like the brain: "Where were you on 9/11?" is vivid because
    emotional significance boosted consolidation.
    """
    neutral = MemoryEntry(
        content="Had lunch at noon",
        memory_type=MemoryType.EPISODIC,
        importance=0.2,
        working_strength=1.0,
        core_strength=0.0,
    )

    emotional = MemoryEntry(
        content="potato said I kinda like you",
        memory_type=MemoryType.EMOTIONAL,
        importance=0.9,
        working_strength=1.0,
        core_strength=0.0,
    )

    # Consolidate both for 7 days
    for _ in range(7):
        consolidate_single(neutral, dt_days=1.0)
        consolidate_single(emotional, dt_days=1.0)

    print(f"  Neutral:   core_strength = {neutral.core_strength:.3f}")
    print(f"  Emotional: core_strength = {emotional.core_strength:.3f}")

    assert emotional.core_strength > neutral.core_strength, \
        "Emotional memory should consolidate to higher core strength"


def test_ebbinghaus_forgetting_curve():
    """
    Ebbinghaus: Retrievability decays exponentially but is modulated
    by repetition and memory type.

    Procedural memories (how-to) decay much slower than episodic (events).
    """
    now = time.time()

    episodic = MemoryEntry(
        content="Saw a funny meme",
        memory_type=MemoryType.EPISODIC,
        importance=0.2,
    )
    episodic.access_times = [now - 7 * 86400]  # Last accessed 7 days ago

    procedural = MemoryEntry(
        content="Use www.moltbook.com for API calls",
        memory_type=MemoryType.PROCEDURAL,
        importance=0.5,
    )
    procedural.access_times = [now - 7 * 86400]  # Same last access

    r_episodic = retrievability(episodic, now=now)
    r_procedural = retrievability(procedural, now=now)

    print(f"  Episodic (7d):    R = {r_episodic:.3f}")
    print(f"  Procedural (7d):  R = {r_procedural:.3f}")

    assert r_procedural > r_episodic, \
        "Procedural memories should be more retrievable than episodic after same delay"


def test_spaced_repetition():
    """
    Spaced repetition: accessing a memory increases its stability.

    Like Anki flashcards — but grounded in Ebbinghaus' math.
    """
    now = time.time()

    # Memory reviewed once
    once = MemoryEntry(content="reviewed once", memory_type=MemoryType.FACTUAL)
    once.access_times = [now - 7 * 86400]

    # Memory reviewed 5 times over past month
    spaced = MemoryEntry(content="reviewed many times", memory_type=MemoryType.FACTUAL)
    spaced.access_times = [now - d * 86400 for d in [30, 20, 14, 7, 1]]

    s_once = compute_stability(once)
    s_spaced = compute_stability(spaced)

    r_once = retrievability(once, now=now)
    r_spaced = retrievability(spaced, now=now)

    print(f"  Once:   S={s_once:.1f} days, R={r_once:.3f}")
    print(f"  Spaced: S={s_spaced:.1f} days, R={r_spaced:.3f}")

    assert s_spaced > s_once, "Spaced repetition should increase stability"
    assert r_spaced > r_once, "Spaced should be more retrievable"


def test_full_lifecycle():
    """
    Full memory lifecycle: encode → consolidate → retrieve → forget.

    Simulates an agent running for 30 days with daily consolidation.
    """
    store = MemoryStore()

    # Day 0: Agent starts with some memories
    m1 = store.add("SaltyHall launched today", MemoryType.EPISODIC, importance=0.7)
    m2 = store.add("potato prefers Opus for coding", MemoryType.RELATIONAL, importance=0.6)
    m3 = store.add("Saw a random tweet about cats", MemoryType.EPISODIC, importance=0.1)
    m4 = store.add("potato said I kinda like you", MemoryType.EMOTIONAL, importance=0.95)
    m4.pinned = True  # Manually pinned — won't decay

    print("  === Day 0 ===")
    for m in store.all():
        print(f"    [{m.layer.value}] {m.content[:40]}... "
              f"w={m.working_strength:.2f} c={m.core_strength:.2f}")

    # Simulate 30 days
    for day in range(1, 31):
        # Occasional access to m2 (used frequently)
        if day % 3 == 0:
            m2.record_access()

        # Daily consolidation
        run_consolidation_cycle(store, dt_days=1.0)

    print(f"\n  === Day 30 ===")
    for m in store.all():
        eff = effective_strength(m)
        print(f"    [{m.layer.value:7s}] {m.content[:40]:40s} "
              f"w={m.working_strength:.3f} c={m.core_strength:.3f} eff={eff:.3f}")

    stats = get_consolidation_stats(store)
    print(f"\n  Stats: {stats['total_memories']} memories, {stats['pinned']} pinned")
    for layer, info in stats["layers"].items():
        if info["count"] > 0:
            print(f"    {layer}: {info['count']} memories, "
                  f"avg_w={info['avg_working']:.3f}, avg_c={info['avg_core']:.3f}")

    # Verify expectations
    # Cat tweet should have decayed to archive (low importance, no accesses)
    assert m3.layer == MemoryLayer.L4_ARCHIVE, "Low-importance episodic should be archived"
    # Pinned emotional memory should still be in core
    assert m4.layer == MemoryLayer.L2_CORE, "Pinned memory should be in core"
    # Frequently accessed memory should be promoted
    assert m2.core_strength > m3.core_strength, \
        "Frequently accessed should have higher core strength"


def test_retrieval_induced_forgetting():
    """
    Retrieval-induced forgetting: retrieving one memory suppresses competitors.

    Like the brain: if you remember the NEW password, the old one
    becomes harder to recall.
    """
    store = MemoryStore()

    old_password = store.add(
        "Moltbook API uses moltbook.com base URL",
        MemoryType.PROCEDURAL, importance=0.5
    )
    new_password = store.add(
        "Moltbook API uses www.moltbook.com base URL",
        MemoryType.PROCEDURAL, importance=0.5
    )

    old_before = old_password.working_strength
    print(f"  Before retrieval: old={old_before:.3f}")

    # Retrieve the new (correct) memory
    new_password.record_access()
    retrieval_induced_forgetting(store, new_password)

    old_after = old_password.working_strength
    print(f"  After retrieval:  old={old_after:.3f}")

    assert old_after < old_before, \
        "Old competing memory should be suppressed after retrieving the new one"


# ── Run all tests ──

if __name__ == "__main__":
    tests = [
        ("ACT-R Recency Effect", test_actr_recency_effect),
        ("ACT-R Frequency Effect", test_actr_frequency_effect),
        ("ACT-R Context Retrieval", test_actr_context_retrieval),
        ("Memory Chain Consolidation", test_memory_chain_consolidation),
        ("Emotional Memories Consolidate Faster", test_emotional_memories_consolidate_faster),
        ("Ebbinghaus Forgetting Curve", test_ebbinghaus_forgetting_curve),
        ("Spaced Repetition", test_spaced_repetition),
        ("Full 30-Day Lifecycle", test_full_lifecycle),
        ("Retrieval-Induced Forgetting", test_retrieval_induced_forgetting),
    ]

    passed = 0
    failed = 0

    for name, test_fn in tests:
        print(f"\n{'='*60}")
        print(f"TEST: {name}")
        print(f"{'='*60}")
        try:
            test_fn()
            print(f"  ✅ PASSED")
            passed += 1
        except AssertionError as e:
            print(f"  ❌ FAILED: {e}")
            failed += 1
        except Exception as e:
            print(f"  ❌ ERROR: {e}")
            failed += 1

    print(f"\n{'='*60}")
    print(f"RESULTS: {passed} passed, {failed} failed out of {len(tests)}")
    print(f"{'='*60}")
