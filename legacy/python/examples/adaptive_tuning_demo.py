#!/usr/bin/env python3
"""
Adaptive Tuning Demo

Shows how adaptive parameter tuning works in practice.
"""

import tempfile
import os
from engram import Memory, MemoryConfig

def main():
    # Create temporary database
    with tempfile.TemporaryDirectory() as tmpdir:
        db_path = os.path.join(tmpdir, "demo.db")
        
        print("=== Adaptive Tuning Demo ===\n")
        
        # Start with chatbot preset + adaptive tuning
        config = MemoryConfig.chatbot()
        print(f"Initial config (chatbot preset):")
        print(f"  min_activation: {config.min_activation}")
        print(f"  context_weight: {config.context_weight}")
        print(f"  mu1 (working decay): {config.mu1}")
        print(f"  mu2 (core decay): {config.mu2}")
        print()
        
        mem = Memory(db_path, config=config, adaptive_tuning=True)
        
        # Make tuner more aggressive for demo purposes
        mem._adaptive_tuner.adaptation_rate = 0.2
        mem._adaptive_tuner.min_samples = 5
        mem._adaptive_tuner.adaptation_interval = 0.0
        
        # Scenario 1: Low hit rate (user keeps searching for things not in memory)
        print("Scenario 1: Simulating low hit rate (empty searches)")
        mem.add("The capital of France is Paris")
        mem.add("Python was created by Guido van Rossum")
        mem.add("The speed of light is 299,792,458 m/s")
        
        # Lots of failed searches
        for _ in range(6):
            mem.recall("quantum physics advanced topics")  # Not in memory
        
        # One success
        mem.recall("capital of France")
        
        stats = mem.stats()
        print(f"  Hit rate: {stats['adaptive_tuning']['hit_rate']:.2%}")
        print(f"  Config adjusted:")
        print(f"    min_activation: {config.min_activation:.2f} (more permissive)")
        print()
        
        # Scenario 2: Negative feedback
        print("Scenario 2: Simulating negative feedback")
        mem.add("The Earth is flat")  # Bad info
        mem.recall("Earth shape")
        mem.reward("That's wrong!")
        mem.reward("Not helpful")
        mem.reward("Bad answer")
        
        # Some positive to have variety
        mem.add("Water boils at 100°C at sea level")
        mem.recall("water boiling point")
        mem.reward("Perfect!")
        
        stats = mem.stats()
        print(f"  Reward ratio: {stats['adaptive_tuning']['reward_ratio']:.2%}")
        print(f"  Config adjusted:")
        print(f"    context_weight: {config.context_weight:.2f} (more context-sensitive)")
        print()
        
        # Scenario 3: High forget rate
        print("Scenario 3: Simulating high forget rate")
        
        # Add many low-importance memories
        for i in range(20):
            mem.add(f"Random fact {i}", importance=0.1)
        
        # Run multiple consolidations with aggressive decay
        old_mu1 = config.mu1
        old_mu2 = config.mu2
        for _ in range(3):
            mem.consolidate(days=5.0)  # Aggressive time step
        
        stats = mem.stats()
        print(f"  Forget rate: {stats['adaptive_tuning']['forget_rate']:.1f} per cycle")
        print(f"  Config adjusted:")
        print(f"    mu1: {config.mu1:.4f} (was {old_mu1:.4f}) - slower decay")
        print(f"    mu2: {config.mu2:.6f} (was {old_mu2:.6f}) - slower decay")
        print()
        
        # Final stats
        print("=== Final Adaptive Metrics ===")
        metrics = stats['adaptive_tuning']
        print(f"Total recalls: {metrics['total_recalls']}")
        print(f"Successful recalls: {metrics['successful_recalls']}")
        print(f"Positive rewards: {metrics['positive_rewards']}")
        print(f"Negative rewards: {metrics['negative_rewards']}")
        print(f"Consolidation cycles: {metrics['consolidation_cycles']}")
        print(f"Memories forgotten: {metrics['memories_forgotten']}")
        print()
        
        print("✓ Demo complete! Parameters adapted based on usage patterns.")

if __name__ == "__main__":
    main()
