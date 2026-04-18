"""
Emotional Bus — Connects Engram to agent workspace files.

The Emotional Bus creates closed-loop feedback between:
- Memory emotions → SOUL updates (drive evolution)
- SOUL drives → Memory importance (what matters)
- Behavior outcomes → HEARTBEAT adjustments (adaptive behavior)

Memory shapes personality. Personality shapes behavior.
Behavior creates new memory. The loop IS the self.
"""

from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Optional
import sqlite3
import re


# === Emotional Accumulator ===

NEGATIVE_THRESHOLD = -0.5
MIN_EVENTS_FOR_SUGGESTION = 10


@dataclass
class EmotionalTrend:
    """Emotional trend for a domain."""
    domain: str
    valence: float       # Running average (-1.0 to 1.0)
    count: int           # Number of emotional events recorded
    last_updated: datetime

    def needs_soul_update(self) -> bool:
        """Check if this trend suggests a need for SOUL update."""
        return self.count >= MIN_EVENTS_FOR_SUGGESTION and self.valence < NEGATIVE_THRESHOLD

    def describe(self) -> str:
        """Describe the trend in human-readable terms."""
        if self.valence > 0.3:
            sentiment = "positive"
        elif self.valence < -0.3:
            sentiment = "negative"
        else:
            sentiment = "neutral"

        return f"{self.domain}: {sentiment} trend ({self.valence:.2f} avg over {self.count} events)"


class EmotionalAccumulator:
    """Emotional accumulator that tracks valence trends per domain."""

    def __init__(self, conn: sqlite3.Connection):
        self.conn = conn
        self._ensure_table()

    def _ensure_table(self):
        """Ensure the emotional_trends table exists."""
        self.conn.execute("""
            CREATE TABLE IF NOT EXISTS emotional_trends (
                domain TEXT PRIMARY KEY,
                valence REAL NOT NULL DEFAULT 0.0,
                count INTEGER NOT NULL DEFAULT 0,
                last_updated REAL NOT NULL
            )
        """)
        self.conn.commit()

    def record_emotion(self, domain: str, valence: float) -> None:
        """
        Record an emotional event for a domain.

        Updates the running average valence for the domain.
        Valence should be in range -1.0 (very negative) to 1.0 (very positive).

        Args:
            domain: Domain/topic (e.g., "coding", "communication")
            valence: Emotional valence (-1.0 to 1.0)
        """
        # Clamp valence to valid range
        valence = max(-1.0, min(1.0, valence))
        now = datetime.now().timestamp()

        # Try to get existing trend
        cursor = self.conn.execute(
            "SELECT valence, count FROM emotional_trends WHERE domain = ?",
            (domain,)
        )
        row = cursor.fetchone()

        if row:
            # Update running average: new_avg = (old_avg * count + new_value) / (count + 1)
            old_valence, count = row
            new_count = count + 1
            new_valence = (old_valence * count + valence) / new_count

            self.conn.execute(
                "UPDATE emotional_trends SET valence = ?, count = ?, last_updated = ? WHERE domain = ?",
                (new_valence, new_count, now, domain)
            )
        else:
            # Insert new trend
            self.conn.execute(
                "INSERT INTO emotional_trends (domain, valence, count, last_updated) VALUES (?, ?, 1, ?)",
                (domain, valence, now)
            )

        self.conn.commit()

    def get_trend(self, domain: str) -> Optional[EmotionalTrend]:
        """Get the emotional trend for a specific domain."""
        cursor = self.conn.execute(
            "SELECT domain, valence, count, last_updated FROM emotional_trends WHERE domain = ?",
            (domain,)
        )
        row = cursor.fetchone()

        if not row:
            return None

        return EmotionalTrend(
            domain=row[0],
            valence=row[1],
            count=row[2],
            last_updated=datetime.fromtimestamp(row[3])
        )

    def get_all_trends(self) -> list[EmotionalTrend]:
        """Get all emotional trends."""
        cursor = self.conn.execute(
            "SELECT domain, valence, count, last_updated FROM emotional_trends ORDER BY count DESC"
        )

        return [
            EmotionalTrend(
                domain=row[0],
                valence=row[1],
                count=row[2],
                last_updated=datetime.fromtimestamp(row[3])
            )
            for row in cursor.fetchall()
        ]

    def get_trends_needing_update(self) -> list[EmotionalTrend]:
        """Get all trends that suggest SOUL updates."""
        return [t for t in self.get_all_trends() if t.needs_soul_update()]

    def reset_trend(self, domain: str) -> None:
        """Reset a domain's trend (after SOUL has been updated)."""
        self.conn.execute("DELETE FROM emotional_trends WHERE domain = ?", (domain,))
        self.conn.commit()

    def decay_trends(self, factor: float) -> int:
        """
        Decay all trends by a factor (used during consolidation).
        This moves trends toward neutral over time.

        Args:
            factor: Decay multiplier (e.g., 0.95)

        Returns:
            Number of trends affected
        """
        now = datetime.now().timestamp()
        cursor = self.conn.execute(
            "UPDATE emotional_trends SET valence = valence * ?, last_updated = ?",
            (factor, now)
        )
        self.conn.commit()
        return cursor.rowcount


# === Drive Alignment ===

ALIGNMENT_BOOST = 1.5


@dataclass
class Drive:
    """A drive/priority from SOUL.md."""
    name: str
    description: str
    keywords: list[str] = field(default_factory=list)

    def extract_keywords(self) -> list[str]:
        """Extract keywords from the drive name and description."""
        keywords = [self.name.lower()]

        # Extract significant words from description (3+ chars, not stopwords)
        stopwords = {"the", "and", "for", "with", "that", "this", "from", "are", "was", "but"}
        words = re.findall(r'\b\w+\b', self.description.lower())
        for word in words:
            if len(word) >= 3 and word not in stopwords:
                keywords.append(word)

        return sorted(set(keywords))


def score_alignment(content: str, drives: list[Drive]) -> float:
    """
    Score how well a memory content aligns with a set of drives.

    Returns a score from 0.0 (no alignment) to 1.0 (strong alignment).

    Args:
        content: The memory content to score
        drives: List of drives to check alignment against

    Returns:
        Alignment score (0.0-1.0)
    """
    if not drives:
        return 0.0

    content_lower = content.lower()
    content_words = set(re.findall(r'\b\w+\b', content_lower))

    total_score = 0.0
    matched_drives = 0

    for drive in drives:
        keywords = drive.keywords if drive.keywords else drive.extract_keywords()

        drive_matches = sum(1 for keyword in keywords if keyword in content_words)

        if drive_matches > 0:
            matched_drives += 1
            # Score contribution: min(1.0, matches / 3) - need at least 3 matches for full score
            drive_score = min(1.0, drive_matches / 3.0)
            total_score += drive_score

    if matched_drives == 0:
        return 0.0

    # Average score across matched drives, capped at 1.0
    return min(1.0, total_score / matched_drives)


def calculate_importance_boost(content: str, drives: list[Drive]) -> float:
    """
    Calculate the importance boost for a memory based on drive alignment.

    Returns a multiplier (1.0 = no boost, ALIGNMENT_BOOST for perfect alignment).

    Args:
        content: The memory content
        drives: List of drives from SOUL.md

    Returns:
        Importance multiplier (1.0 to ALIGNMENT_BOOST)
    """
    alignment = score_alignment(content, drives)

    if alignment <= 0.0:
        return 1.0

    # Linear interpolation between 1.0 and ALIGNMENT_BOOST based on alignment
    return 1.0 + (ALIGNMENT_BOOST - 1.0) * alignment


def find_aligned_drives(content: str, drives: list[Drive]) -> list[tuple[str, float]]:
    """
    Find which drives a piece of content aligns with.

    Args:
        content: The memory content
        drives: List of drives

    Returns:
        List of (drive_name, alignment_score) pairs for aligned drives, sorted by score descending
    """
    content_lower = content.lower()
    content_words = set(re.findall(r'\b\w+\b', content_lower))

    aligned = []

    for drive in drives:
        keywords = drive.keywords if drive.keywords else drive.extract_keywords()
        matches = sum(1 for keyword in keywords if keyword in content_words)

        if matches > 0:
            score = min(1.0, matches / 3.0)
            aligned.append((drive.name, score))

    # Sort by score descending
    aligned.sort(key=lambda x: x[1], reverse=True)
    return aligned


# === Behavior Feedback ===

LOW_SCORE_THRESHOLD = 0.2
MIN_ATTEMPTS_FOR_SUGGESTION = 10
DEFAULT_SCORE_WINDOW = 20


@dataclass
class BehaviorLog:
    """A logged behavior outcome."""
    action: str
    outcome: bool       # True = positive, False = negative
    timestamp: datetime


@dataclass
class ActionStats:
    """Statistics for an action."""
    action: str
    total: int
    positive: int
    negative: int
    score: float        # Positive rate (positive / total)

    def should_deprioritize(self) -> bool:
        """Check if this action should be deprioritized."""
        return self.total >= MIN_ATTEMPTS_FOR_SUGGESTION and self.score < LOW_SCORE_THRESHOLD

    def describe(self) -> str:
        """Describe the action performance in human-readable terms."""
        if self.score >= 0.8:
            rating = "excellent"
        elif self.score >= 0.5:
            rating = "moderate"
        elif self.score >= 0.2:
            rating = "poor"
        else:
            rating = "very poor"

        return f"{self.action}: {rating} ({self.score * 100:.0f}% success rate, {self.positive}/{self.total} positive)"


class BehaviorFeedback:
    """Behavior feedback tracker."""

    def __init__(self, conn: sqlite3.Connection):
        self.conn = conn
        self._ensure_table()

    def _ensure_table(self):
        """Ensure the behavior_log table exists."""
        self.conn.executescript("""
            CREATE TABLE IF NOT EXISTS behavior_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                action TEXT NOT NULL,
                outcome INTEGER NOT NULL,
                timestamp REAL NOT NULL
            );
            
            CREATE INDEX IF NOT EXISTS idx_behavior_action ON behavior_log(action);
            CREATE INDEX IF NOT EXISTS idx_behavior_timestamp ON behavior_log(timestamp);
        """)
        self.conn.commit()

    def log_outcome(self, action: str, positive: bool) -> None:
        """
        Log an action outcome.

        Args:
            action: Name of the action (e.g., "check_email", "run_consolidation")
            positive: Whether the outcome was positive
        """
        self.conn.execute(
            "INSERT INTO behavior_log (action, outcome, timestamp) VALUES (?, ?, ?)",
            (action, int(positive), datetime.now().timestamp())
        )
        self.conn.commit()

    def get_action_score(self, action: str, window: int = DEFAULT_SCORE_WINDOW) -> Optional[float]:
        """
        Get the success score for an action over a window of recent attempts.

        Args:
            action: Action name
            window: Number of recent attempts to consider

        Returns:
            Success rate (positive / total) or None if no history
        """
        cursor = self.conn.execute(
            "SELECT outcome FROM behavior_log WHERE action = ? ORDER BY timestamp DESC LIMIT ?",
            (action, window)
        )
        outcomes = [bool(row[0]) for row in cursor.fetchall()]

        if not outcomes:
            return None

        return sum(outcomes) / len(outcomes)

    def get_action_stats(self, action: str) -> Optional[ActionStats]:
        """Get full statistics for an action."""
        cursor = self.conn.execute(
            "SELECT COUNT(*), SUM(outcome) FROM behavior_log WHERE action = ?",
            (action,)
        )
        row = cursor.fetchone()

        if not row or row[0] == 0:
            return None

        total = row[0]
        positive = row[1] or 0
        negative = total - positive

        return ActionStats(
            action=action,
            total=total,
            positive=positive,
            negative=negative,
            score=positive / total if total > 0 else 0.0
        )

    def get_all_action_stats(self) -> list[ActionStats]:
        """Get all action statistics."""
        cursor = self.conn.execute(
            "SELECT action, COUNT(*), SUM(outcome) FROM behavior_log GROUP BY action ORDER BY COUNT(*) DESC"
        )

        stats = []
        for row in cursor.fetchall():
            action = row[0]
            total = row[1]
            positive = row[2] or 0
            negative = total - positive

            stats.append(ActionStats(
                action=action,
                total=total,
                positive=positive,
                negative=negative,
                score=positive / total if total > 0 else 0.0
            ))

        return stats

    def get_actions_to_deprioritize(self) -> list[ActionStats]:
        """Get actions that should be deprioritized."""
        return [s for s in self.get_all_action_stats() if s.should_deprioritize()]

    def get_successful_actions(self, min_score: float = 0.8) -> list[ActionStats]:
        """Get actions with high success rate."""
        all_stats = self.get_all_action_stats()
        return [
            s for s in all_stats
            if s.total >= MIN_ATTEMPTS_FOR_SUGGESTION and s.score >= min_score
        ]

    def clear_action(self, action: str) -> int:
        """
        Clear all logs for an action (e.g., after adjusting HEARTBEAT).

        Returns:
            Number of logs deleted
        """
        cursor = self.conn.execute(
            "DELETE FROM behavior_log WHERE action = ?",
            (action,)
        )
        self.conn.commit()
        return cursor.rowcount


# === Workspace File I/O ===

@dataclass
class HeartbeatTask:
    """A task from HEARTBEAT.md with completion status."""
    description: str
    completed: bool
    original_line: str


@dataclass
class Identity:
    """Identity fields from IDENTITY.md."""
    name: Optional[str] = None
    creature: Optional[str] = None
    vibe: Optional[str] = None
    emoji: Optional[str] = None


def parse_soul(content: str) -> list[Drive]:
    """
    Parse SOUL.md to extract drives/priorities.

    Looks for:
    - `key: value` pairs
    - `- item` bullet points (treated as drives)
    - Section headers starting with `#` are used as context
    """
    drives = []
    current_section = ""

    for line in content.splitlines():
        trimmed = line.strip()

        # Track section headers
        if trimmed.startswith('#'):
            current_section = trimmed.lstrip('#').strip()
            continue

        # Skip empty lines
        if not trimmed:
            continue

        # Parse key: value pairs
        if ':' in trimmed:
            parts = trimmed.split(':', 1)
            key = parts[0].strip()
            value = parts[1].strip()

            # Skip if key looks like a URL or is empty
            if '/' not in key and key and value:
                drive = Drive(name=key, description=value)
                drive.keywords = drive.extract_keywords()
                drives.append(drive)
                continue

        # Parse bullet points
        if trimmed.startswith(('-', '*')):
            item = trimmed[1:].strip()
            if item:
                # Use section as context if available
                if current_section:
                    name = f"{current_section}/{' '.join(item.split()[:3])}"
                else:
                    name = ' '.join(item.split()[:3])

                drive = Drive(name=name, description=item)
                drive.keywords = drive.extract_keywords()
                drives.append(drive)

    return drives


def parse_heartbeat(content: str) -> list[HeartbeatTask]:
    """
    Parse HEARTBEAT.md to extract tasks with completion status.

    Looks for:
    - `- [ ] task` (uncompleted)
    - `- [x] task` or `- [X] task` (completed)
    """
    tasks = []

    for line in content.splitlines():
        trimmed = line.strip()

        # Parse checkbox items
        if trimmed.startswith("- ["):
            match = re.match(r'^- \[(.)\] (.+)$', trimmed)
            if match:
                checkbox_content = match.group(1)
                description = match.group(2).strip()
                completed = checkbox_content.upper() == 'X'

                if description:
                    tasks.append(HeartbeatTask(
                        description=description,
                        completed=completed,
                        original_line=line
                    ))

    return tasks


def parse_identity(content: str) -> Identity:
    """
    Parse IDENTITY.md to extract identity fields.

    Looks for:
    - `name: value`
    - `creature: value`
    - `vibe: value`
    - `emoji: value`
    """
    identity = Identity()

    for line in content.splitlines():
        if ':' in line:
            parts = line.split(':', 1)
            key = parts[0].strip().lower()
            value = parts[1].strip()

            if not value:
                continue

            if key == "name":
                identity.name = value
            elif key == "creature":
                identity.creature = value
            elif key == "vibe":
                identity.vibe = value
            elif key == "emoji":
                identity.emoji = value

    return identity


def read_soul(workspace_dir: Path) -> list[Drive]:
    """Read and parse SOUL.md from workspace directory."""
    path = workspace_dir / "SOUL.md"
    if not path.exists():
        return []
    return parse_soul(path.read_text())


def read_heartbeat(workspace_dir: Path) -> list[HeartbeatTask]:
    """Read and parse HEARTBEAT.md from workspace directory."""
    path = workspace_dir / "HEARTBEAT.md"
    if not path.exists():
        return []
    return parse_heartbeat(path.read_text())


def read_identity(workspace_dir: Path) -> Identity:
    """Read and parse IDENTITY.md from workspace directory."""
    path = workspace_dir / "IDENTITY.md"
    if not path.exists():
        return Identity()
    return parse_identity(path.read_text())


# === Main Emotional Bus ===

@dataclass
class SoulUpdate:
    """A suggested update to SOUL.md based on emotional trends."""
    domain: str
    action: str         # e.g., "add drive", "modify drive", "note pattern"
    content: str
    trend: EmotionalTrend


@dataclass
class HeartbeatUpdate:
    """A suggested update to HEARTBEAT.md based on behavior feedback."""
    action: str
    suggestion: str     # e.g., "deprioritize", "boost", "remove"
    stats: ActionStats


class EmotionalBus:
    """The Emotional Bus — main interface for emotional feedback loops."""

    def __init__(self, workspace_dir: str, conn: sqlite3.Connection):
        """
        Create a new Emotional Bus.

        Args:
            workspace_dir: Path to the agent workspace containing SOUL.md, HEARTBEAT.md, etc.
            conn: SQLite database connection (shared with Memory)
        """
        self.workspace_dir = Path(workspace_dir)
        self.conn = conn

        # Initialize components
        self.accumulator = EmotionalAccumulator(conn)
        self.feedback = BehaviorFeedback(conn)

        # Load drives from SOUL.md
        self.drives = read_soul(self.workspace_dir)

    def reload_drives(self) -> None:
        """Reload drives from SOUL.md."""
        self.drives = read_soul(self.workspace_dir)

    def process_interaction(self, content: str, emotion: float, domain: str) -> None:
        """
        Process an interaction with emotional content.

        This is the main entry point for the emotional feedback loop.
        Call this when storing a memory with emotional significance.

        Args:
            content: The memory content
            emotion: Emotional valence (-1.0 to 1.0)
            domain: The domain/topic (e.g., "coding", "communication")
        """
        self.accumulator.record_emotion(domain, emotion)

    def align_importance(self, content: str) -> float:
        """
        Calculate importance boost for a memory based on drive alignment.

        Call this when storing a memory to potentially boost its importance.

        Returns:
            Importance multiplier (1.0 = no boost, up to ALIGNMENT_BOOST)
        """
        return calculate_importance_boost(content, self.drives)

    def alignment_score(self, content: str) -> float:
        """Score how well content aligns with drives."""
        return score_alignment(content, self.drives)

    def find_aligned(self, content: str) -> list[tuple[str, float]]:
        """Find which drives a piece of content aligns with."""
        return find_aligned_drives(content, self.drives)

    def log_behavior(self, action: str, positive: bool) -> None:
        """Log a behavior outcome."""
        self.feedback.log_outcome(action, positive)

    def get_trends(self) -> list[EmotionalTrend]:
        """Get emotional trends."""
        return self.accumulator.get_all_trends()

    def get_behavior_stats(self) -> list[ActionStats]:
        """Get behavior statistics."""
        return self.feedback.get_all_action_stats()

    def suggest_soul_updates(self) -> list[SoulUpdate]:
        """
        Suggest SOUL updates based on accumulated emotional trends.

        Returns suggestions when domains have accumulated enough negative
        or positive patterns to warrant drive adjustments.
        """
        trends_needing_update = self.accumulator.get_trends_needing_update()
        suggestions = []

        for trend in trends_needing_update:
            if trend.valence < -0.7:
                suggestion = SoulUpdate(
                    domain=trend.domain,
                    action="add drive",
                    content=f"Avoid {trend.domain} approaches that consistently lead to negative outcomes",
                    trend=trend
                )
            elif trend.valence < NEGATIVE_THRESHOLD:
                suggestion = SoulUpdate(
                    domain=trend.domain,
                    action="note pattern",
                    content=f"Be cautious with {trend.domain} - showing signs of friction ({trend.valence:.2f} avg over {trend.count} events)",
                    trend=trend
                )
            else:
                continue

            suggestions.append(suggestion)

        # Also suggest reinforcing very positive trends
        all_trends = self.accumulator.get_all_trends()
        for trend in all_trends:
            if trend.count >= MIN_EVENTS_FOR_SUGGESTION and trend.valence > 0.7:
                suggestions.append(SoulUpdate(
                    domain=trend.domain,
                    action="reinforce",
                    content=f"Continue {trend.domain} - consistently positive outcomes ({trend.valence:.2f} avg over {trend.count} events)",
                    trend=trend
                ))

        return suggestions

    def suggest_heartbeat_updates(self) -> list[HeartbeatUpdate]:
        """
        Suggest HEARTBEAT updates based on behavior feedback.

        Returns suggestions for actions that should be deprioritized
        or boosted based on their historical success rates.
        """
        suggestions = []

        # Actions to deprioritize
        for stats in self.feedback.get_actions_to_deprioritize():
            suggestions.append(HeartbeatUpdate(
                action=stats.action,
                suggestion="deprioritize",
                stats=stats
            ))

        # Actions doing well (suggest boosting)
        for stats in self.feedback.get_successful_actions(0.8):
            suggestions.append(HeartbeatUpdate(
                action=stats.action,
                suggestion="boost",
                stats=stats
            ))

        return suggestions

    def get_identity(self) -> Identity:
        """Get the current identity from workspace."""
        return read_identity(self.workspace_dir)

    def get_heartbeat_tasks(self) -> list[HeartbeatTask]:
        """Get heartbeat tasks from workspace."""
        return read_heartbeat(self.workspace_dir)
