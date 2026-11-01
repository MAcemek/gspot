export function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean).slice(0, 2);
  const s = parts.map((p) => p[0]!.toUpperCase()).join("");
  return s || "G";
}

export function wordCount(text: string): number {
  return (text.match(/[\p{L}\p{N}'’-]+/gu) ?? []).length;
}

export type DiffOp = { kind: "same" | "del" | "ins"; text: string };

/** Word-level diff (LCS) for showing a rewrite against the original. */
export function wordDiff(a: string, b: string): DiffOp[] {
  const A = a.split(/(\s+)/).filter((x) => x.length > 0);
  const B = b.split(/(\s+)/).filter((x) => x.length > 0);
  const n = A.length, m = B.length;
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i]![j] = A[i] === B[j] ? dp[i + 1]![j + 1]! + 1 : Math.max(dp[i + 1]![j]!, dp[i]![j + 1]!);
  const out: DiffOp[] = [];
  let i = 0, j = 0;
  const push = (kind: DiffOp["kind"], text: string) => {
    const last = out[out.length - 1];
    if (last && last.kind === kind) last.text += text; else out.push({ kind, text });
  };
  while (i < n && j < m) {
    if (A[i] === B[j]) { push("same", A[i]!); i++; j++; }
    else if (dp[i + 1]![j]! >= dp[i]![j + 1]!) { push("del", A[i]!); i++; }
    else { push("ins", B[j]!); j++; }
  }
  while (i < n) push("del", A[i++]!);
  while (j < m) push("ins", B[j++]!);
  return out;
}

export function relativeTime(ts: number, now = Date.now() / 1000): string {
  const d = Math.max(0, now - ts);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)} min ago`;
  if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
  return `${Math.floor(d / 86400)} d ago`;
}

export function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n - 1) + "…" : s;
}

export const LANGUAGES = [
  "English (American)", "English (British)", "Spanish", "French", "German", "Italian", "Portuguese (Brazil)", "Portuguese (Portugal)",
  "Dutch", "Swedish", "Norwegian", "Danish", "Finnish", "Polish", "Czech", "Hungarian", "Romanian", "Greek", "Turkish", "Russian",
  "Ukrainian", "Hebrew", "Arabic", "Hindi", "Bengali", "Urdu", "Tamil", "Telugu", "Indonesian", "Malay", "Vietnamese", "Thai",
  "Chinese (Simplified)", "Chinese (Traditional)", "Japanese", "Korean", "Filipino", "Swahili",
];

export const PARAPHRASE_STYLES = [
  { id: "Humanize", desc: "Natural, varied, no AI clichés" },
  { id: "Academic", desc: "Formal, precise, third person" },
  { id: "Professional", desc: "Clear, polite, business-ready" },
  { id: "Streamlined", desc: "Shorter, same meaning" },
  { id: "Creative", desc: "Vivid and expressive" },
  { id: "Simple", desc: "Plain words, short sentences" },
  { id: "Expand", desc: "Add detail and transitions" },
  { id: "Shorten", desc: "Cut to the essentials" },
];

export const VOICES = [
  { id: "naturalist", name: "The Naturalist", desc: "Replace common AI patterns.", glyph: "face", guide: "Write the way a thoughtful person talks: varied sentence length, concrete details, occasional asides, contractions, no stacked adjectives, no hollow transitions, no AI clichés.", isNew: true },
  { id: "precisionist", name: "The Precisionist", desc: "Communicate efficiently.", glyph: "scissors", guide: "Be economical. Short sentences, active voice, one idea per sentence, no filler, no intensifiers, precise verbs." },
  { id: "everyday", name: "The Everyday Voice", desc: "Sound clear and balanced in any context.", glyph: "cup", guide: "Clear, friendly, balanced. Contractions are fine. Plain words. Warm but not gushing." },
  { id: "executive", name: "The Executive", desc: "Be confident and assertive at work.", glyph: "compass", guide: "Confident, direct, decisive. Lead with the point. No hedging. Crisp business vocabulary without jargon." },
  { id: "scholar", name: "The Scholar", desc: "Use formal, objective language for school.", glyph: "quill", guide: "Formal, objective, third person, measured claims, precise terminology, no contractions, no slang." },
];

export const AUDIENCES = [
  { id: "followers", name: "Online followers", desc: "People who follow you on social platforms; skim fast, respond to authenticity and clear takeaways." },
  { id: "general", name: "General readers", desc: "Curious adults with no special background." },
  { id: "manager", name: "My manager", desc: "Busy, outcome-focused, wants status, risks and asks." },
  { id: "team", name: "Teammates", desc: "Peers who know the context and want specifics and next steps." },
  { id: "customers", name: "Customers", desc: "People deciding whether to trust and buy; care about benefits and proof." },
  { id: "investors", name: "Investors", desc: "Evaluate traction, team, market and risk; ask hard questions." },
  { id: "professor", name: "Professor", desc: "Expects structure, evidence and precise claims." },
  { id: "friends", name: "Friends", desc: "Informal, supportive, react to feeling and story." },
];

/** What a pasted key looks like, from its prefix (the Rust side verifies it). */
export function keyKind(key: string): string {
  const k = key.trim();
  if (!k) return "";
  if (k.startsWith("sk-ant-")) return "Anthropic key";
  if (k.startsWith("sk-or-")) return "OpenRouter key";
  if (k.startsWith("gsk_")) return "Groq key";
  if (k.startsWith("AIza")) return "Google AI key";
  if (k.startsWith("nvapi-")) return "NVIDIA API key";
  if (k.startsWith("sk-")) return "OpenAI key";
  if (k.startsWith("pk_")) return "publik key (use the publik API card instead)";
  return "API key";
}
