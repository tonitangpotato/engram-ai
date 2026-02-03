"""
Synaptic Downscaling — Homeostatic Plasticity

Neuroscience basis: During sleep (especially slow-wave sleep), the brain
performs global synaptic downscaling — all synaptic weights are reduced
by a proportional factor. This was proposed by Tononi & Cirelli (2003, 2006)
as the Synaptic Homeostasis Hypothesis (SHY).

Key properties:
- Preserves relative ordering of memory strengths
- Prevents unbounded growth from continuous encoding
- Effectively "raises the bar" for what counts as a strong memory
- Weak memories drop below retrieval threshold → functionally forgotten

Think of it as renormalization: after a day of learning (synaptic potentiation),
sleep "compresses" the scale so the system stays in a useful dynamic range.

Without downscaling, every memory would eventually saturate at maximum strength,
destroying the signal (important vs unimportant memories become indistinguishable).

References:
- Tononi & Cirelli (2003) — Sleep and synaptic homeostasis hypothesis
- Tononi & Cirelli (2006) — Sleep function and synaptic homeostasis
"""

import sys
import os


from engram.core import MemoryStore


def synaptic_downscale(store, factor: float = 0.95):
    """
    Multiply all memory strengths by factor.

    Preserves relative ordering while compressing absolute values.
    Pinned memories are exempt (they represent manually protected knowledge).

    Typical usage: call after each consolidation cycle with factor=0.95
    (5% reduction per "sleep"), or periodically with a larger factor.

    Args:
        store: MemoryStore to downscale
        factor: Multiplicative factor (0-1). 0.95 = 5% reduction.

    Returns:
        dict with stats: {n_scaled, avg_before, avg_after}
    """
    assert 0.0 < factor <= 1.0, f"Factor must be in (0, 1], got {factor}"

    memories = store.all()
    if not memories:
        return {"n_scaled": 0, "avg_before": 0.0, "avg_after": 0.0}

    total_before = 0.0
    total_after = 0.0
    n_scaled = 0

    for entry in memories:
        if entry.pinned:
            continue

        before = entry.working_strength + entry.core_strength
        total_before += before

        entry.working_strength *= factor
        entry.core_strength *= factor

        after = entry.working_strength + entry.core_strength
        total_after += after
        n_scaled += 1

        _update = getattr(store, 'update', None)
        if _update: _update(entry)

    return {
        "n_scaled": n_scaled,
        "avg_before": total_before / max(n_scaled, 1),
        "avg_after": total_after / max(n_scaled, 1),
    }


if __name__ == "__main__":
    """Demo: downscaling effect over multiple cycles."""
    from engram.core import MemoryType

    store = MemoryStore()
    m1 = store.add("Important fact", MemoryType.FACTUAL, importance=0.8)
    m1.working_strength = 1.0
    m1.core_strength = 0.5

    m2 = store.add("Trivial fact", MemoryType.EPISODIC, importance=0.1)
    m2.working_strength = 0.3
    m2.core_strength = 0.1

    m3 = store.add("Pinned memory", MemoryType.EMOTIONAL, importance=0.9)
    m3.pinned = True
    m3.working_strength = 1.0
    m3.core_strength = 0.8

    print("=== Synaptic Downscaling Demo ===\n")
    print("  Simulating 10 sleep cycles with factor=0.95:\n")

    for cycle in range(11):
        if cycle > 0:
            synaptic_downscale(store, factor=0.95)

        if cycle % 2 == 0:
            print(f"  Cycle {cycle:2d}:")
            for m in store.all():
                total = m.working_strength + m.core_strength
                pin = " [PINNED]" if m.pinned else ""
                print(f"    total={total:.4f} (w={m.working_strength:.4f} c={m.core_strength:.4f})"
                      f" | {m.content[:30]}{pin}")
            print()
