import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppState, KeyProbe, PublikStatus, Settings, Session, UpdateInfo } from "./types";

// Outside Tauri (a plain browser, used for layout screenshots and tests) the
// commands answer from a fixture instead of the host.
export const hasTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const invoke: typeof tauriInvoke = hasTauri
  ? tauriInvoke
  : (async (cmd: string, args?: Record<string, unknown>) => (await import("./mock")).mockInvoke(cmd, args)) as typeof tauriInvoke;

export const ipc = {
  getState: () => invoke<AppState>("get_state"),
  acceptAlert: (id: string, suggestion = 0) => invoke<{ method: string; new_text: string }>("accept_alert", { id, suggestion }),
  dismissAlert: (id: string) => invoke<void>("dismiss_alert", { id }),
  replaceRange: (start: number, end: number, text: string) => invoke<{ method: string; new_text: string }>("replace_range", { start, end, text }),
  runFeature: <T = any>(name: string, args: Record<string, unknown> = {}) => invoke<T>("run_feature", { name, args }),
  chatSend: (reqId: string, messages: { role: string; content: string }[]) => invoke<string>("chat_send", { reqId, messages }),
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  addUserWord: (word: string) => invoke<string[]>("add_user_word", { word }),
  removeUserWord: (word: string) => invoke<string[]>("remove_user_word", { word }),
  getHistory: () => invoke<Session[]>("get_history"),
  openPanel: (view?: string, focus = false) => invoke<void>("open_panel", { view, focus }),
  hidePanel: () => invoke<void>("hide_panel"),
  tuckPanel: () => invoke<void>("tuck_panel"),
  permissionPrompt: () => invoke<boolean>("permission_prompt"),
  permissionStatus: () => invoke<boolean>("permission_status"),
  setCardRects: (rects: { x: number; y: number; w: number; h: number }[]) => invoke<void>("set_card_rects", { rects }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  publikStatus: () => invoke<PublikStatus>("publik_status"),
  publikAccept: () => invoke<PublikStatus>("publik_accept_disclosure"),
  publikDismissFirstRun: () => invoke<PublikStatus>("publik_dismiss_first_run"),
  publikForget: () => invoke<PublikStatus>("publik_forget"),
  publikRefresh: () => invoke<PublikStatus>("publik_refresh"),
  probeKey: (key: string) => invoke<KeyProbe>("probe_key", { key }),
  checkUpdate: () => invoke<UpdateInfo | null>("check_update"),
  installUpdate: () => invoke<void>("install_update"),
  appVersion: () => invoke<string>("app_version"),
  quit: () => invoke<void>("quit_app"),
};

export function on<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (!hasTauri) return Promise.resolve(() => {});
  return listen<T>(event, (e) => cb(e.payload));
}
