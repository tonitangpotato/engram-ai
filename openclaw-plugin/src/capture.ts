/**
 * Engram Context Engine — Auto-Capture
 *
 * Detects messages worth storing as long-term memories.
 * Uses heuristics (no LLM call) to keep it zero-cost.
 */

export type CaptureDecision = {
  shouldCapture: boolean;
  memoryType: "episodic" | "semantic" | "procedural" | "preference";
  importance: number;
  content: string;
};

/**
 * Analyze a message and decide if it should be captured as a memory.
 *
 * We look for:
 * - User preferences ("I like...", "I prefer...", "Don't...")
 * - Factual statements ("My name is...", "I work at...")
 * - Decisions ("Let's use...", "We decided...")
 * - Corrections ("Actually...", "No, it should be...")
 * - Code patterns (important technical context)
 */
export function analyzeForCapture(
  role: string,
  content: string,
): CaptureDecision {
  const no: CaptureDecision = {
    shouldCapture: false,
    memoryType: "episodic",
    importance: 0,
    content: "",
  };

  if (!content || content.length < 10) return no;
  if (role !== "user" && role !== "assistant") return no;

  const lower = content.toLowerCase();

  // Skip heartbeats, no-reply, short acks
  if (lower.includes("heartbeat_ok") || lower.trim() === "no_reply") return no;

  // CJK text is denser — 15 CJK chars ≈ 30 Latin chars of meaning
  const hasCjk = /[\u4e00-\u9fff\u3040-\u309f\u30a0-\u30ff]/.test(content);
  const minLength = hasCjk ? 10 : 30;
  if (content.length < minLength && !lower.includes("?") && !lower.includes("？")) return no;

  // Preferences
  const prefPatterns = [
    /\b(i (?:like|prefer|want|need|hate|love|dislike|always|never))\b/i,
    /\b(don'?t (?:like|want|use|do))\b/i,
    /(我(?:喜欢|想要|不想|讨厌|偏好|习惯|总是|从不))/,
  ];
  for (const pat of prefPatterns) {
    if (pat.test(content)) {
      return {
        shouldCapture: true,
        memoryType: "preference",
        importance: 0.7,
        content: truncate(content, 500),
      };
    }
  }

  // Facts / Identity
  const factPatterns = [
    /\b(my name is|i (?:am|work|live|have|use|run))\b/i,
    /\b((?:we|our) (?:use|have|run|built|deploy))\b/i,
    /(我(?:是|叫|在|有|用))/,
  ];
  for (const pat of factPatterns) {
    if (pat.test(content)) {
      return {
        shouldCapture: true,
        memoryType: "semantic",
        importance: 0.6,
        content: truncate(content, 500),
      };
    }
  }

  // Decisions
  const decisionPatterns = [
    /\b(let'?s (?:use|go with|do|try|switch|change))\b/i,
    /\b(we (?:decided|agreed|should|will|chose))\b/i,
    /\b(from now on|going forward|the plan is)\b/i,
    /(以后|从现在|我们决定|计划是)/,
  ];
  for (const pat of decisionPatterns) {
    if (pat.test(content)) {
      return {
        shouldCapture: true,
        memoryType: "procedural",
        importance: 0.8,
        content: truncate(content, 500),
      };
    }
  }

  // Corrections — high importance, these fix misconceptions
  const corrPatterns = [
    /\b(actually|no,? (?:it|that|this)|wrong|incorrect|not right)\b/i,
    /\b(the correct|should be|fix(?:ed)?:)\b/i,
    /(其实|不对|应该是|错了|正确的)/,
  ];
  for (const pat of corrPatterns) {
    if (pat.test(content)) {
      return {
        shouldCapture: true,
        memoryType: "semantic",
        importance: 0.85,
        content: truncate(content, 500),
      };
    }
  }

  // Assistant insights — only capture substantial ones
  if (role === "assistant" && content.length > 200) {
    // Key learnings, summaries
    if (
      /\b(key (?:insight|learning|takeaway)|in summary|the main)\b/i.test(
        content,
      ) ||
      /(关键|总结|核心|重要的是)/.test(content)
    ) {
      return {
        shouldCapture: true,
        memoryType: "semantic",
        importance: 0.65,
        content: truncate(content, 500),
      };
    }
  }

  return no;
}

function truncate(s: string, maxLen: number): string {
  if (s.length <= maxLen) return s;
  // Safe truncation for multi-byte (CJK) strings
  let end = maxLen;
  // Walk back to avoid splitting a multi-byte char
  while (end > 0 && s.charCodeAt(end) >= 0xdc00 && s.charCodeAt(end) <= 0xdfff) {
    end--;
  }
  return s.slice(0, end) + "…";
}
