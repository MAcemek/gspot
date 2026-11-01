import { useCallback, useEffect, useRef, useState } from "react";
import { ipc, on } from "../lib/ipc";
import type { Alert, AppState, Doc, PublikStatus, Score, Session, Settings, Status, UpdateInfo, View } from "../lib/types";
import * as I from "../components/icons";
import { Review } from "./views/Review";
import { Chat } from "./views/Chat";
import { Performance } from "./views/Performance";
import { Tone } from "./views/Tone";
import { Paraphraser, Humanizer, CeoMode, Reactions } from "./views/Rewrite";
import { Translate, Explain, Rewriter, Detector, Plagiarism } from "./views/Analyze";
import { History, SettingsView } from "./views/Misc";
import { Onboarding } from "./views/Onboarding";

export interface Ctx {
  doc: Doc | null; alerts: Alert[]; status: Status; settings: Settings; score: Score | null; history: Session[]; initials: string;
  publik: PublikStatus; spot_running: boolean;
  view: View; setView: (v: View) => void; toast: (m: string) => void; refresh: () => void;
  updateSettings: (patch: Partial<Settings>) => Promise<void>;
}

const RAIL: { id: View; label: string; Icon: (p: any) => JSX.Element }[] = [
  { id: "chat", label: "AI Chat", Icon: I.ChatSpark },
  { id: "plagiarism", label: "Plagiarism", Icon: I.Incognito },
  { id: "tone", label: "Tone Detector", Icon: I.ToneWave },
  { id: "performance", label: "Performance Score", Icon: I.Target },
  { id: "paraphraser", label: "Paraphraser", Icon: I.Paraphrase },
  { id: "humanizer", label: "Humanizer", Icon: I.Heart },
  { id: "ceo", label: "Talk like a CEO", Icon: I.Briefcase },
  { id: "reactions", label: "Reader Reactions", Icon: I.Book },
  { id: "translate", label: "Translate", Icon: I.Translate },
  { id: "explain", label: "Explain It", Icon: I.Glasses },
  { id: "rewriter", label: "AI Rewriter", Icon: I.Bolt },
  { id: "detector", label: "AI Detector", Icon: I.Scanner },
];

function initialView(): View {
  try {
    const v = new URLSearchParams(window.location.search).get("view");
    if (v) return v as View;
  } catch { /* no URL in some webviews */ }
  return "review";
}

export function Panel() {
  const [state, setState] = useState<AppState | null>(null);
  const [view, setView] = useState<View>(initialView);
  const [toastMsg, setToastMsg] = useState<string | null>(null);
  const toastTimer = useRef<number | null>(null);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [installing, setInstalling] = useState(false);

  const refresh = useCallback(() => { ipc.getState().then(setState).catch(() => {}); }, []);
  const onboardingShown = useRef(false);

  useEffect(() => {
    refresh();
    const subs = [
      on<Doc>("host://document", (doc) => setState((s) => (s ? { ...s, doc } : s))),
      on<Alert[]>("host://alerts", (alerts) => setState((s) => (s ? { ...s, alerts } : s))),
      on<Status>("host://status", (status) => setState((s) => (s ? { ...s, status } : s))),
      on<{ start: number; end: number; text: string }>("host://selection", (sel) => setState((s) => (s && s.doc ? { ...s, doc: { ...s.doc, sel_start: sel.start, sel_end: sel.end, selected_text: sel.text } } : s))),
      on<{ view: View }>("panel://view", (p) => setView(p.view)),
      on<Settings>("settings://changed", (settings) => setState((s) => (s ? { ...s, settings } : s))),
      on<string>("ai://notice", (m) => toast(m)),
      on<UpdateInfo>("update://available", (u) => setUpdate(u)),
    ];
    const t = window.setInterval(refresh, 4000);
    return () => { subs.forEach((p) => p.then((f) => f())); window.clearInterval(t); };
  }, [refresh]);

  // Score follows alerts + doc
  useEffect(() => {
    if (!state?.doc) return;
    ipc.runFeature<Score>("score").then((score) => setState((s) => (s ? { ...s, score } : s))).catch(() => {});
  }, [state?.doc?.version, state?.alerts.length]);

  const toast = useCallback((m: string) => {
    setToastMsg(m);
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToastMsg(null), 2800);
  }, []);

  const updateSettings = useCallback(async (patch: Partial<Settings>) => {
    const cur = state?.settings ?? (await ipc.getSettings());
    const next = { ...cur, ...patch };
    await ipc.setSettings(next);
    setState((s) => (s ? { ...s, settings: next } : s));
  }, [state?.settings]);

  // First run: once Accessibility is granted, the AI picker replaces the review list until a choice is made.
  useEffect(() => {
    if (state && state.status.trusted && !state.settings.onboarded && !onboardingShown.current) { onboardingShown.current = true; setView("onboarding"); }
  }, [state?.status.trusted, state?.settings.onboarded]);

  if (!state) return <div className="panel"><div className="panel__content" style={{ alignItems: "center", justifyContent: "center" }}><div className="spin" /></div></div>;

  const ctx: Ctx = { ...state, view, setView, toast, refresh, updateSettings };
  const railLeft = state.settings.rail_side === "left";

  return (
    <div className={"panel" + (railLeft ? " rail-left" : "")}>
      <div className="panel__content">
        {update && (
          <div className="notice" style={{ display: "flex", alignItems: "center", gap: 8, margin: "8px 12px 0" }}>
            <span style={{ flex: 1 }}>GSpot {update.version} is available</span>
            <button className="btn btn--primary" style={{ padding: "4px 10px" }} disabled={installing} onClick={() => { setInstalling(true); ipc.installUpdate().catch((e) => { toast(String(e)); setInstalling(false); }); }}>{installing ? "Installing…" : "Install and restart"}</button>
            <button className="btn btn--ghost" style={{ padding: "4px 8px" }} title="Dismiss" onClick={() => setUpdate(null)}>×</button>
          </div>
        )}
        {!state.status.trusted ? <PermissionGate /> : <ViewSwitch ctx={ctx} />}
        {toastMsg && <div className="toast">{toastMsg}</div>}
      </div>
      <div className="rail">
        <div className="rail__top">
          <button className="rail__btn" title="History" onClick={() => setView("history")}><I.Clock /><span className="rail__label">History</span></button>
        </div>
        <div className="rail__stack">
          <button className={"rail__btn" + (view === "review" ? " active" : "")} onClick={() => setView("review")}>
            <I.GMark style={{ color: "#2cc9b6" }} />
            {state.alerts.length > 0 && <span className="badge">{state.alerts.length > 99 ? "99+" : state.alerts.length}</span>}
            <span className="rail__label">GSpot</span>
          </button>
          {RAIL.map(({ id, label, Icon }) => (
            <button key={id} className={"rail__btn" + (view === id ? " active" : "")} onClick={() => setView(id)}>
              <Icon /><span className="rail__label">{label}</span>
            </button>
          ))}
          <button className="rail__btn" onClick={() => setView("settings")}><I.ChevronDown /><span className="rail__label">Settings</span></button>
        </div>
        <div className="rail__bottom">
          <button className="rail__btn" title="Tuck away" onClick={() => ipc.tuckPanel()}>
            {state.settings.dock_side === "right" ? <I.ChevronRight /> : <I.ChevronLeft />}
            <span className="rail__label">Tuck away</span>
          </button>
          <button className="avatar" title={state.settings.user_name} onClick={() => setView("settings")}>{state.initials}</button>
        </div>
      </div>
    </div>
  );
}

function ViewSwitch({ ctx }: { ctx: Ctx }) {
  switch (ctx.view) {
    case "review": return <Review ctx={ctx} />;
    case "chat": return <Chat ctx={ctx} />;
    case "plagiarism": return <Plagiarism ctx={ctx} />;
    case "tone": return <Tone ctx={ctx} />;
    case "performance": return <Performance ctx={ctx} />;
    case "paraphraser": return <Paraphraser ctx={ctx} />;
    case "humanizer": return <Humanizer ctx={ctx} />;
    case "ceo": return <CeoMode ctx={ctx} />;
    case "reactions": return <Reactions ctx={ctx} />;
    case "translate": return <Translate ctx={ctx} />;
    case "explain": return <Explain ctx={ctx} />;
    case "rewriter": return <Rewriter ctx={ctx} />;
    case "detector": return <Detector ctx={ctx} />;
    case "history": return <History ctx={ctx} />;
    case "settings": return <SettingsView ctx={ctx} />;
    case "onboarding": return <Onboarding ctx={ctx} />;
  }
}

function PermissionGate() {
  const [checking, setChecking] = useState(false);
  return (
    <div className="panel__scroll">
      <div className="hero">
        <div className="hero__icon"><I.Shield /></div>
        <h1>Turn on Accessibility</h1>
        <p>GSpot reads the text field you are typing in and draws suggestions on top of it. macOS needs you to allow that once.</p>
      </div>
      <ol style={{ color: "var(--text-subdued)", fontSize: 15, lineHeight: 1.7, paddingLeft: 20 }}>
        <li>Click <b>Open System Settings</b>.</li>
        <li>Switch on <b>GSpot</b> under Privacy &amp; Security → Accessibility.</li>
        <li>Come back here. This panel updates by itself.</li>
      </ol>
      <div className="row-actions">
        <button className="btn btn--primary" disabled={checking} onClick={async () => { setChecking(true); await ipc.permissionPrompt(); setChecking(false); }}>Open System Settings</button>
        <button className="btn btn--ghost" onClick={() => ipc.openUrl("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")}>Open the Accessibility list</button>
      </div>
      <div className="notice" style={{ marginTop: 18 }}>If the switch is already on, turn it off and on again after a rebuild. macOS keys the grant to the app's signature.</div>
    </div>
  );
}

export function Hero({ icon, title, children }: { icon: JSX.Element; title: string; children?: React.ReactNode }) {
  return (
    <div className="hero">
      <div className="hero__icon">{icon}</div>
      <h1>{title}</h1>
      {children}
    </div>
  );
}

export function TitleBar({ title, right, onReload }: { title: string; right?: React.ReactNode; onReload?: () => void }) {
  return (
    <div className="titlebar">
      {title}
      <div className="titlebar__actions">
        {onReload && <button title="Reload" onClick={onReload}><I.Refresh /></button>}
        {right}
      </div>
    </div>
  );
}

export function Empty({ icon, title, children }: { icon: JSX.Element; title: string; children?: React.ReactNode }) {
  return (
    <div className="empty">
      <div className="empty__art">{icon}</div>
      <h2>{title}</h2>
      <p>{children}</p>
    </div>
  );
}

export function NoDoc({ ctx }: { ctx: Ctx }) {
  const st = ctx.status;
  if (st.skipped && st.frontmost) {
    const resume = () => ctx.updateSettings({ skip_apps: ctx.settings.skip_apps.filter((a) => a.bundle_id !== st.frontmost_bid) }).catch((e) => ctx.toast(String(e)));
    const unskip = () => ctx.updateSettings({ unskip_apps: [...ctx.settings.unskip_apps, st.frontmost_bid] }).catch((e) => ctx.toast(String(e)));
    return (
      <Empty icon={<I.Pencil />} title={`GSpot is off in ${st.frontmost}`}>
        {st.skip_built_in
          ? <>Terminals and code editors show commands and code, not writing, so GSpot stays out of them. <button className="linkbtn" onClick={unskip}>Turn it on in {st.frontmost}</button></>
          : <>You switched GSpot off in this app. <button className="linkbtn" onClick={resume}>Turn it on in {st.frontmost}</button></>}
      </Empty>
    );
  }
  return (
    <Empty icon={<I.Pencil />} title="Click into a text field">
      Open any app and click where you write. GSpot follows your cursor{ctx.status.frontmost ? ` (now in front: ${ctx.status.frontmost})` : ""}.
    </Empty>
  );
}

export function Steps({ steps, active }: { steps: string[]; active: number }) {
  return (
    <div className="steps">
      {steps.map((s, i) => (
        <div key={s} className={i < active ? "done" : i === active ? "done" : ""} style={{ display: "flex", gap: 10, alignItems: "center" }}>
          {i < active ? <I.Check style={{ width: 16, height: 16, color: "#2cc9b6" }} /> : i === active ? <div className="spin" style={{ width: 14, height: 14, borderWidth: 2 }} /> : <span style={{ width: 16, display: "inline-block" }} />}
          {s}
        </div>
      ))}
    </div>
  );
}

/** Runs a feature with fake-but-honest step progress while the real call is in flight. */
export function useFeature<T>(name: string, steps: string[]) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [step, setStep] = useState(0);
  const run = useCallback(async (args: Record<string, unknown> = {}) => {
    setLoading(true); setError(null); setStep(0);
    const t = window.setInterval(() => setStep((s) => Math.min(s + 1, steps.length - 1)), 1400);
    try {
      const r = await ipc.runFeature<T>(name, args);
      setData(r);
      return r;
    } catch (e: any) {
      setError(String(e)); return null;
    } finally { window.clearInterval(t); setLoading(false); setStep(steps.length); }
  }, [name, steps.length]);
  return { data, error, loading, step, run, reset: () => { setData(null); setError(null); } };
}
