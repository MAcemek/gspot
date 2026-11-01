import { useEffect, useState } from "react";
import type { Ctx } from "../Panel";
import * as I from "../../components/icons";
import { TitleBar, Empty } from "../Panel";
import { ipc } from "../../lib/ipc";
import { relativeTime } from "../../lib/text";
import type { Settings, UpdateInfo } from "../../lib/types";
import { AiSetup } from "./AiSetup";

export function History({ ctx }: { ctx: Ctx }) {
  return (
    <>
      <TitleBar title="History" />
      <div className="panel__scroll">
        {ctx.history.length === 0 ? <Empty icon={<I.Clock />} title="Nothing yet">Documents you write in show up here.</Empty> : ctx.history.map((h) => (
          <div className="hist" key={h.doc_id}><b>{h.title}</b><span>{h.app_name} · {h.words} words{h.score != null ? ` · score ${h.score}` : ""} · {relativeTime(h.last_seen)}</span></div>
        ))}
      </div>
    </>
  );
}

export function SettingsView({ ctx }: { ctx: Ctx }) {
  const [s, setS] = useState<Settings>(ctx.settings);
  useEffect(() => { setS(ctx.settings); }, [ctx.settings]);
  const save = async (next: Settings) => { setS(next); await ctx.updateSettings(next); };
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [busy, setBusy] = useState<"" | "checking" | "installing">("");
  useEffect(() => { ipc.appVersion().then(setVersion).catch(() => {}); }, []);
  const check = async () => {
    setBusy("checking");
    try {
      const u = await ipc.checkUpdate();
      setUpdate(u);
      if (!u) ctx.toast("GSpot is up to date.");
    } catch (e) { ctx.toast(String(e)); }
    setBusy("");
  };
  const install = async () => {
    setBusy("installing");
    try { await ipc.installUpdate(); } catch (e) { ctx.toast(String(e)); setBusy(""); }
  };
  return (
    <>
      <TitleBar title="Settings" />
      <div className="panel__scroll">
        <div className="section-label">AI</div>
        <AiSetup ctx={ctx} mode="settings" />
        <div className="section-label">General</div>
        <label className="kv"><span>Start GSpot with Windows</span><input type="checkbox" checked={s.start_with_windows ?? true} onChange={(e) => save({ ...s, start_with_windows: e.target.checked })} /></label>
        <div className="kv">
          <span>GSpot {version}{update && <><br /><span style={{ color: "var(--text-subdued)", fontSize: 13 }}>Version {update.version} is available</span></>}</span>
          {update
            ? <button className="btn btn--primary" disabled={busy !== ""} onClick={install}>{busy === "installing" ? "Installing…" : "Install and restart"}</button>
            : <button className="btn btn--outline" disabled={busy !== ""} onClick={check}>{busy === "checking" ? "Checking…" : "Check for updates"}</button>}
        </div>
        <div className="section-label">Suggestions</div>
        <label className="kv"><span>Spot-check sentences when I stop typing</span><input type="checkbox" checked={s.auto_spot_check} onChange={(e) => save({ ...s, auto_spot_check: e.target.checked })} /></label>
        <label className="kv"><span>Show the G button next to text fields</span><input type="checkbox" checked={s.show_gbutton} onChange={(e) => save({ ...s, show_gbutton: e.target.checked })} /></label>
        <label className="kv"><span>Draw underlines in the app</span><input type="checkbox" checked={s.show_underlines} onChange={(e) => save({ ...s, show_underlines: e.target.checked })} /></label>
        <div className="row-actions"><button className="btn btn--outline" onClick={() => save({ ...s, dismissed: [] })}>Restore dismissed suggestions ({s.dismissed.length})</button></div>
        <div className="section-label">Skipped apps</div>
        <p style={{ color: "var(--text-subdued)", fontSize: 13, margin: "2px 0 8px" }}>GSpot stays out of terminals and code editors. Add other apps with "Skip this app" in the Review view.</p>
        {s.skip_apps.length === 0 ? <p style={{ color: "var(--text-subdued)", fontSize: 13, margin: "0 0 8px" }}>No apps added.</p> : s.skip_apps.map((a) => (
          <div className="kv" key={a.bundle_id}><span>{a.name || a.bundle_id}</span><button className="btn btn--outline" style={{ padding: "4px 10px" }} onClick={() => save({ ...s, skip_apps: s.skip_apps.filter((x) => x.bundle_id !== a.bundle_id) })}>Turn on</button></div>
        ))}
        <p style={{ color: "var(--text-subdued)", fontSize: 13, margin: "0 0 8px" }}>Code editors and terminals are skipped automatically.</p>
        {ctx.status.skip_built_in && ctx.status.skipped && ctx.status.frontmost_bid && (
          <div className="row-actions"><button className="btn btn--outline" onClick={() => ctx.updateSettings({ unskip_apps: [...(s.unskip_apps ?? []), ctx.status.frontmost_bid] }).catch((e) => ctx.toast(String(e)))}>Turn GSpot on in {ctx.status.frontmost}</button></div>
        )}
        {(s.unskip_apps ?? []).map((id) => (
          <div className="kv" key={id}><span>{id}</span><button className="btn btn--outline" style={{ padding: "4px 10px" }} title="Skip this app again" onClick={() => save({ ...s, unskip_apps: (s.unskip_apps ?? []).filter((x) => x !== id) })}>Skip again ×</button></div>
        ))}
        <div className="section-label">Your dictionary</div>
        {(s.user_words ?? []).length === 0 ? <p style={{ color: "var(--text-subdued)", fontSize: 13, margin: "2px 0 8px" }}>Words you add from a spelling card show up here.</p> : (s.user_words ?? []).map((w) => (
          <div className="kv" key={w}><span>{w}</span><button className="btn btn--outline" style={{ padding: "4px 10px" }} title="Remove from dictionary" onClick={() => ipc.removeUserWord(w).then(() => ctx.refresh()).catch((e) => ctx.toast(String(e)))}>×</button></div>
        ))}
        <div className="section-label">You</div>
        <div className="field"><label>Name (used for the avatar and greetings)</label><input value={s.user_name} onChange={(e) => setS({ ...s, user_name: e.target.value })} onBlur={() => save(s)} /></div>
        <div className="section-label">Panel</div>
        <div className="field"><label>Dock side</label><div className="select-wrap"><select className="select" value={s.dock_side} onChange={(e) => save({ ...s, dock_side: e.target.value as any })}><option value="left">Left</option><option value="right">Right</option></select></div></div>
        <div className="field"><label>Icon rail</label><div className="select-wrap"><select className="select" value={s.rail_side} onChange={(e) => save({ ...s, rail_side: e.target.value as any })}><option value="right">Right (like Grammarly)</option><option value="left">Left</option></select></div></div>
        <div className="section-label">Plagiarism search</div>
        <p style={{ color: "var(--text-subdued)", fontSize: 13, margin: "2px 0 8px" }}>Plagiarism checks search the web through publik API when it is on, one cent a search. Paste your own key below to search with it instead.</p>
        <div className="field"><label>Brave Search API key (for plagiarism checking)</label><input type="password" placeholder="Optional" value={s.search.brave_key} onChange={(e) => setS({ ...s, search: { ...s.search, brave_key: e.target.value } })} onBlur={() => save(s)} /></div>
        <div className="section-label">App</div>
        <div className="row-actions"><button className="btn btn--outline" onClick={() => ipc.permissionPrompt()}>Accessibility permission</button><button className="btn btn--ghost" onClick={() => ipc.quit()}>Quit GSpot</button></div>
      </div>
    </>
  );
}
