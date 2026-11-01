export type Category = "correctness" | "clarity" | "engagement" | "delivery";

export interface Rect { x: number; y: number; w: number; h: number }

export interface Doc {
  id: string; app_name: string; bundle_id: string; pid: number; role: string;
  text: string; sel_start: number; sel_end: number; selected_text: string;
  frame: Rect; visible: Rect; version: number; active: boolean; truncated: boolean;
}

export interface Alert {
  id: string; category: Category; kind: string; title: string; message: string;
  start: number; end: number; u16_start: number; u16_end: number; original: string;
  suggestions: string[]; priority: number; stable_hash: number;
}

export interface Status { trusted: boolean; frontmost: string; frontmost_bid: string; skipped: boolean; skip_built_in: boolean; has_doc: boolean; linting: boolean }
export interface SkipApp { bundle_id: string; name: string }

export interface Score {
  words: number; sentences: number; characters: number; reading_time_sec: number; flesch: number;
  overall: number | null; correctness_count: number; clarity_count: number; engagement_count: number; delivery_count: number;
  correctness_label: string; clarity_label: string; engagement_label: string; delivery_label: string;
  correctness_fill: number; clarity_fill: number; engagement_fill: number; delivery_fill: number; note: string | null;
}

export interface Voice { id: string; name: string; description: string; custom: boolean }
export interface Audience { id: string; name: string; description: string; custom: boolean }

/** "ollama" is the local model (Settings → AI → Local AI), the default in this build. */
export type ProviderId = "ollama" | "openai" | "anthropic" | "publik";
export interface LlmConfig { provider: ProviderId; base_url: string; api_key: string; model: string }
export interface PublikSettings { disclosure_version: number; first_run_owed: boolean }

export interface Settings {
  llm: LlmConfig; dock_side: "left" | "right"; rail_side: "left" | "right"; user_name: string;
  search: { brave_key: string }; custom_voices: Voice[]; custom_audiences: Audience[];
  dismissed: number[]; target_language: string; citation_style: string; show_gbutton: boolean; show_underlines: boolean; onboarded: boolean;
  auto_spot_check: boolean; ai_fallback_local: boolean; publik: PublikSettings; skip_apps: SkipApp[];
}

export interface PublikNotice { kind: string; message: string; link_label: string; link_url: string }
export interface PublikStatus {
  available: boolean; can_provision: boolean; has_key: boolean; selected: boolean;
  state: "not_set_up" | "disconnected" | "needs_credit" | "unreachable" | "ready";
  claim_state: string; balance_micros: number | null; balance_line: string; last_charge_label: string | null; week_label: string | null;
  model: string; model_served: string | null; justification: string; disclosure: { cost: string; data: string; terms_url: string };
  cta: { label: string; url: string; claimed: boolean }; later_label: string; first_run_owed: boolean; dashboard_url: string; notice: PublikNotice | null;
}
export interface KeyProbe { provider: ProviderId; base_url: string; model: string; models: number; reply: string }
export interface SpotResult { busy?: boolean; stale?: boolean; sentences: number; checked: number; from_cache: number; batches: number; skipped: number; rejected: number; suggestions: number; ms: number }

export interface Session { doc_id: string; app_name: string; bundle_id: string; title: string; words: number; score: number | null; last_seen: number }

export interface AppState { doc: Doc | null; alerts: Alert[]; status: Status; settings: Settings; history: Session[]; score: Score | null; initials: string; publik: PublikStatus; spot_running: boolean }

export type View =
  | "review" | "chat" | "plagiarism" | "tone" | "performance" | "paraphraser" | "humanizer" | "ceo"
  | "reactions" | "translate" | "explain" | "rewriter" | "detector" | "history" | "settings" | "onboarding";

export interface UnderlineRect { id: string; x: number; y: number; w: number; h: number; color: string; category: string; sentence: boolean }
export interface Geometry { origin_x: number; origin_y: number; width: number; height: number; rects: UnderlineRect[]; hovered: string | null; over_card: boolean; }
