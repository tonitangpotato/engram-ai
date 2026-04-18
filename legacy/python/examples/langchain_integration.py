#!/usr/bin/env python3
"""
Example: LangChain Integration with neuromemory-ai

Shows how to use neuromemory-ai as a LangChain memory backend.

Note: Requires langchain and openai packages:
    pip install langchain openai

Run: OPENAI_API_KEY=your-key python examples/langchain_integration.py
"""

import os
import sys
import tempfile
from typing import Any

# Add parent directory for imports
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from engram import Memory
from engram.config import MemoryConfig

# Check if LangChain is available
try:
    from langchain.memory.chat_memory import BaseChatMemory
    from langchain.schema import BaseMessage, HumanMessage, AIMessage
    LANGCHAIN_AVAILABLE = True
except ImportError:
    LANGCHAIN_AVAILABLE = False
    print("⚠️  LangChain not installed. Install with: pip install langchain")
    print("    This example shows the integration pattern.\n")


class NeuromemoryLangChainMemory:
    """
    LangChain-compatible memory using neuromemory-ai.
    
    This provides cognitive memory dynamics (ACT-R activation, 
    Hebbian learning, consolidation) to LangChain agents.
    """
    
    def __init__(
        self, 
        db_path: str = "./langchain_memory.db",
        config: MemoryConfig = None,
        memory_key: str = "history",
        return_messages: bool = False,
    ):
        self.mem = Memory(db_path, config=config or MemoryConfig.chatbot())
        self.memory_key = memory_key
        self.return_messages = return_messages
        self._recent_human_input = None
    
    @property
    def memory_variables(self) -> list[str]:
        """Variables this memory provides to prompts."""
        return [self.memory_key]
    
    def load_memory_variables(self, inputs: dict) -> dict:
        """Load relevant memories for the current input."""
        # Get the human input from the inputs dict
        human_input = inputs.get("input", inputs.get("question", ""))
        self._recent_human_input = human_input
        
        # Recall relevant memories
        results = self.mem.recall(human_input, limit=5, min_confidence=0.3)
        
        if self.return_messages:
            # Return as message objects (for chat models)
            messages = []
            for r in results:
                content = r["content"]
                if content.startswith("Human: "):
                    messages.append(HumanMessage(content=content[7:]))
                elif content.startswith("AI: "):
                    messages.append(AIMessage(content=content[4:]))
                else:
                    # Fact/preference - add as system context
                    messages.append(AIMessage(content=f"[Memory] {content}"))
            return {self.memory_key: messages}
        else:
            # Return as string (for completion models)
            history = "\n".join(f"- {r['content']}" for r in results)
            return {self.memory_key: history}
    
    def save_context(self, inputs: dict, outputs: dict) -> None:
        """Save the interaction to memory."""
        human_input = inputs.get("input", inputs.get("question", ""))
        ai_output = outputs.get("output", outputs.get("answer", outputs.get("text", "")))
        
        # Store the exchange as episodic memory
        if human_input:
            self.mem.add(
                f"Human: {human_input}",
                type="episodic",
                importance=0.4
            )
        
        if ai_output:
            self.mem.add(
                f"AI: {ai_output[:500]}",  # Truncate long responses
                type="episodic",
                importance=0.3
            )
    
    def add_user_preference(self, preference: str, importance: float = 0.7):
        """Add a user preference to long-term memory."""
        self.mem.add(preference, type="relational", importance=importance)
    
    def add_fact(self, fact: str, importance: float = 0.6):
        """Add a fact to memory."""
        self.mem.add(fact, type="factual", importance=importance)
    
    def consolidate(self, days: float = 1.0):
        """Run memory consolidation (call periodically)."""
        self.mem.consolidate(days=days)
    
    def clear(self) -> None:
        """Clear is a no-op - memories naturally decay instead."""
        pass
    
    def get_stats(self) -> dict:
        """Get memory statistics."""
        return self.mem.stats()


def demo_without_langchain():
    """Demonstrate the memory pattern without requiring LangChain."""
    print("=" * 60)
    print("LangChain Integration Pattern Demo")
    print("(Simulating LangChain without the actual package)")
    print("=" * 60)
    
    with tempfile.NamedTemporaryFile(suffix=".db", delete=False) as f:
        db_path = f.name
    
    try:
        # Create memory
        memory = NeuromemoryLangChainMemory(db_path=db_path)
        
        # Add some user preferences
        print("\n📝 Adding user preferences to memory:")
        memory.add_user_preference("User prefers Python code examples")
        print("   + User prefers Python code examples")
        memory.add_user_preference("User is a senior developer")
        print("   + User is a senior developer")
        memory.add_fact("Python 3.12 was released in October 2023")
        print("   + Python 3.12 was released in October 2023")
        
        # Simulate conversation
        print("\n💬 Simulating conversation:")
        
        exchanges = [
            ("How do I read a file in Python?", "You can use `with open('file.txt') as f: content = f.read()`"),
            ("What about async file reading?", "Use `aiofiles` library: `async with aiofiles.open('file.txt') as f: ...`"),
            ("Can you show exception handling?", "Wrap in try/except: `try: ... except FileNotFoundError: ...`"),
        ]
        
        for human, ai in exchanges:
            print(f"\n   Human: {human}")
            print(f"   AI: {ai[:80]}...")
            
            # Save to memory (like LangChain would)
            memory.save_context({"input": human}, {"output": ai})
        
        # Consolidate (simulate time passing)
        print("\n💤 Running consolidation...")
        memory.consolidate(days=0.5)
        
        # Test recall
        print("\n🔍 Testing memory recall:")
        
        queries = [
            "file reading Python",
            "async programming",
            "user preferences",
        ]
        
        for query in queries:
            print(f"\n   Query: '{query}'")
            result = memory.load_memory_variables({"input": query})
            history = result["history"]
            if history:
                for line in history.split("\n")[:3]:
                    print(f"      {line}")
            else:
                print("      (no relevant memories)")
        
        # Show stats
        stats = memory.get_stats()
        print("\n📊 Memory Statistics:")
        print(f"   Total memories: {stats['total_memories']}")
        
        print("\n✅ Demo complete!")
        print("\n💡 To use with actual LangChain:")
        print("   from langchain.chains import ConversationChain")
        print("   from langchain.llms import OpenAI")
        print("   chain = ConversationChain(llm=OpenAI(), memory=NeuromemoryLangChainMemory())")
        
    finally:
        os.unlink(db_path)


def demo_with_langchain():
    """Full LangChain demo (requires langchain package)."""
    print("=" * 60)
    print("Full LangChain Integration Demo")
    print("=" * 60)
    
    # This would use actual LangChain if available
    # For now, show the code pattern
    
    code = '''
from langchain.chains import ConversationChain
from langchain.llms import OpenAI

# Create memory with cognitive features
memory = NeuromemoryLangChainMemory(
    db_path="./agent.db",
    config=MemoryConfig.personal_assistant()
)

# Add user context
memory.add_user_preference("User is named Alice")
memory.add_user_preference("User prefers detailed explanations")

# Create chain with cognitive memory
chain = ConversationChain(
    llm=OpenAI(temperature=0.7),
    memory=memory,
    verbose=True
)

# Chat!
response = chain.predict(input="Hello! What can you help me with?")
print(response)

# The memory automatically:
# - Retrieves relevant context using ACT-R activation
# - Stores the conversation as episodic memory
# - Forms Hebbian links between related topics
# - Consolidates over time (call memory.consolidate() periodically)
'''
    
    print("\n📝 Example code:")
    print(code)


def main():
    if LANGCHAIN_AVAILABLE:
        demo_with_langchain()
    else:
        demo_without_langchain()


if __name__ == "__main__":
    main()
