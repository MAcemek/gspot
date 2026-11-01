// Fixture state for the panel outside Tauri (layout screenshots, tests).
// Nothing here reaches the host; `?mock=…` picks a scenario.
import type { Alert, AppState, Category, PublikStatus, Score, Settings } from "./types";

const TEXT = [
  "So here is what actually happened. After the video blew up i kinda realized that school wasnt bringing me any closer to my goals. I had an audience, but no money.",
  "i wish i could say that i felt some sort of conviction, but honestly, it was stranger than anything i had ever experienced, and with every day that went by without results i couldnt help but question if i had made the right choice.",
  "In order to be able to make progress at this point in time, it was decided by me that the best course of action would be to send a very large number of cold emails to founders. The response was very good.",
].join("\n\n");

function range(needle: string): { start: number; end: number } {
  const start = TEXT.indexOf(needle);
  if (start < 0) throw new Error("fixture needle missing: " + needle);
  return { start, end: start + needle.length };
}

let n = 0;
function alert(category: Category, kind: string, title: string, message: string, needle: string, suggestions: string[], priority = 100): Alert {
  const r = range(needle);
  n += 1;
  return { id: `mock-${n}`, category, kind, title, message, start: r.start, end: r.end, u16_start: r.start, u16_end: r.end, original: needle, suggestions, priority, stable_hash: n };
}

const LONG = "i wish i could say that i felt some sort of conviction, but honestly, it was stranger than anything i had ever experienced, and with every day that went by without results i couldnt help but question if i had made the right choice.";
const WORDY = "In order to be able to make progress at this point in time, it was decided by me that the best course of action would be to send a very large number of cold emails to founders.";

const ALERTS: Alert[] = [
  alert("correctness", "Capitalization", "Capitalize the word", "The pronoun I is always capitalized.", "i kinda", ["I kinda"]),
  alert("correctness", "Spelling", "Correct your spelling", "This word is misspelled.", "wasnt", ["wasn't"]),
  alert("correctness", "Spelling", "Correct your spelling", "This word is misspelled.", "couldnt", ["couldn't"]),
  alert("clarity", "SpotRephrase", "Split the long sentence", "This sentence runs 44 words. Two shorter sentences make the same points and read faster.", LONG,
    ["I wish I could say I felt some conviction, but honestly, it was stranger than anything I had ever experienced. With every day that went by without results, I couldn't help but question whether I had made the right choice."], 150),
  alert("clarity", "SpotRephrase", "Make it more concise", "Plain words say the same thing in less space.", WORDY, ["To make progress now, I decided to send many cold emails to founders."], 150),
  alert("clarity", "Readability", "Split the long sentence", "This sentence is 44 words long. The split keeps every word. Accept it, or break the sentence where you prefer.", LONG, [LONG.replace(", and with every", ". With every")]),
  alert("engagement", "Enhancement", "Make it more engaging", "A stronger word says more than very + good.", "very good", ["excellent"]),
  alert("delivery", "Delivery", "Use a formal word", "Kinda sounds casual in writing.", "kinda", ["somewhat"]),
];

const SCORE: Score = {
  words: 118, sentences: 7, characters: 640, reading_time_sec: 28, flesch: 72, overall: 71,
  correctness_count: 3, clarity_count: 3, engagement_count: 1, delivery_count: 1,
  correctness_label: "3 alerts", clarity_label: "3 alerts", engagement_label: "1 alert", delivery_label: "1 alert",
  correctness_fill: 62, clarity_fill: 48, engagement_fill: 85, delivery_fill: 88, note: null,
};

const SETTINGS: Settings = {
  llm: { provider: "publik", base_url: "https://publikhq.com/api/v1", api_key: "", model: "publik-balanced" },
  dock_side: "left", rail_side: "right", user_name: "Mann Bellani", search: { brave_key: "" },
  custom_voices: [], custom_audiences: [], dismissed: [], target_language: "Spanish", citation_style: "MLA",
  show_gbutton: true, show_underlines: true, onboarded: true, auto_spot_check: true, ai_fallback_local: true, publik: { disclosure_version: 1, first_run_owed: false }, skip_apps: [],
};

const PUBLIK: PublikStatus = {
  available: true, can_provision: true, has_key: false, selected: false, state: "not_set_up", claim_state: "anonymous",
  balance_micros: null, balance_line: "", last_charge_label: null, week_label: null, model: "publik-balanced", model_served: null,
  justification: "The model that reads your sentences runs on a server, and the server time costs money.",
  disclosure: { cost: "$0.00 to start. GSpot includes $0.05 of free use, once.", data: "Only the text you check is sent.", terms_url: "https://publikhq.com/terms" },
  cta: { label: "Sign in with publik", url: "https://publikhq.com/claim", claimed: false }, later_label: "Later", first_run_owed: false,
  dashboard_url: "https://publikhq.com/dashboard", notice: null,
};

function scenario(): string {
  try { return new URLSearchParams(window.location.search).get("mock") ?? "doc"; } catch { return "doc"; }
}

function state(): AppState {
  const s = scenario();
  const doc = s === "nodoc" || s === "terminal" ? null : {
    id: "mock-doc", app_name: "Notes", bundle_id: "com.apple.Notes", pid: 1, role: "AXTextArea", text: TEXT,
    sel_start: 0, sel_end: 0, selected_text: "", frame: { x: 0, y: 0, w: 600, h: 400 }, visible: { x: 0, y: 0, w: 600, h: 400 },
    version: 1, active: true, truncated: false,
  };
  return {
    doc, alerts: s === "clean" || !doc ? [] : ALERTS, status: { trusted: true, frontmost: s === "terminal" ? "Terminal" : "Notes", frontmost_bid: s === "terminal" ? "com.apple.Terminal" : "com.apple.Notes", skipped: s === "terminal", skip_built_in: s === "terminal", has_doc: !!doc, linting: false },
    settings: { ...SETTINGS, onboarded: s !== "onboarding" }, history: [], score: doc ? SCORE : null, initials: "MB", publik: PUBLIK, spot_running: s === "spotting",
  };
}

export async function mockInvoke(cmd: string, args?: Record<string, unknown>): Promise<any> {
  switch (cmd) {
    case "get_state": return state();
    case "get_settings": return state().settings;
    case "set_settings": return;
    case "get_history": return [];
    case "publik_status": case "publik_accept_disclosure": case "publik_dismiss_first_run": case "publik_forget": case "publik_refresh": return PUBLIK;
    case "run_feature": {
      const name = String(args?.name ?? "");
      if (name === "score") return SCORE;
      if (name === "tone") return { tones: [{ name: "Confident", confidence: 0.72 }, { name: "Informal", confidence: 0.55 }, { name: "Direct", confidence: 0.4 }], summary: "The writing comes across as confident and direct, with a casual voice." };
      if (name === "spot_check") return { sentences: 7, checked: 7, from_cache: 0, batches: 1, skipped: 0, rejected: 0, suggestions: 2, ms: 1200 };
      return { too_short: true };
    }
    case "accept_alert": return { method: "ax", new_text: TEXT };
    case "ui_ready": case "ui_log": case "set_card_rects": case "open_url": case "dismiss_alert": case "open_panel": case "hide_panel": case "tuck_panel": return;
    case "permission_status": case "permission_prompt": return true;
    default: return {};
  }
}
