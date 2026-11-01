import { useEffect, useState } from "react";
import type { Ctx } from "../Panel";
import { ipc } from "../../lib/ipc";
import { keyKind } from "../../lib/text";
import type { LlmConfig, PublikStatus } from "../../lib/types";
import * as I from "../../components/icons";

type Choice = "local" | "publik" | "key";

const FREE_KEYS = [
  { name: "Groq", url: "https://console.groq.com/keys", note: "Free · fastest · about 1,000 requests a day" },
  { name: "Google Gemini", url: "https://aistudio.google.com/apikey", note: "Free · Google may use free-tier text to improve its products" },
  { name: "NVIDIA", url: "https://build.nvidia.com/", note: "Free for developers · about 40 requests a minute · for testing, not production" },
  { name: "OpenRouter", url: "https://openrouter.ai/keys", note: "50 free requests a day · hundreds of models" },
  { name: "Claude (paid)", url: "https://console.anthropic.com/settings/keys", note: "Paid · very accurate · a fraction of a cent per use" },
];

/** Which card is on: local AI once a model is picked, publik API once its key exists, your own key once one is pasted, else none. */
function currentChoice(llm: LlmConfig, publik: PublikStatus): Choice | null {
  if (llm.provider === "ollama") return llm.model.trim() ? "local" : null;
  if (llm.provider === "publik") return publik.has_key ? "publik" : null;
  return llm.api_key.trim().length > 0 ? "key" : null;
}

/**
 * The AI picker shared by onboarding and Settings. Three cards, no URLs or
 * ports: local AI (Ollama on this computer, private and free), publik API
 * (zero setup) and your own key (one field, provider detected). The plumbing
 * sits behind "Advanced".
 */
export function AiSetup({ ctx, mode, onDone }: { ctx: Ctx; mode: "onboarding" | "settings"; onDone?: () => void }) {
  const llm = ctx.settings.llm;
  const publik = ctx.publik;
  const choice = currentChoice(llm, publik);
  const keyConnected = choice === "key";
  const [open, setOpen] = useState<Choice | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [key, setKey] = useState("");
  const [ping, setPing] = useState<string | null>(null);

  useEffect(() => { if (choice === "publik" && publik.has_key) ipc.publikRefresh().catch(() => {}); }, [choice, publik.has_key]);

  const finish = () => { if (mode === "onboarding") onDone?.(); };

  const turnOnPublik = async () => {
    setBusy(true); setError(null);
    try {
      // The first-run card (balance line, justification, "Sign in with publik") must be seen
      // before any usage; onboarding ends from that card's buttons, not here.
      const st = await ipc.publikAccept(); ctx.refresh(); setOpen(null); ctx.toast("publik API is on");
      if (!st.first_run_owed) finish();
    }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  const connectKey = async () => {
    setBusy(true); setError(null);
    try {
      const r = await ipc.probeKey(key);
      await ctx.updateSettings({ llm: { provider: r.provider, base_url: r.base_url, api_key: key.trim(), model: r.model } });
      setKey(""); setOpen(null); ctx.toast(`Connected · ${r.model}`); finish();
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  const publikState = publik.state === "ready" ? "ready" : publik.state === "needs_credit" ? "needs a plan or pack" : publik.state === "unreachable" ? "unreachable right now" : publik.state === "disconnected" ? "disconnected" : "";

  return (
    <>
      {/* Local AI */}
      <button className={"choice" + (choice === "local" ? " selected" : "")} onClick={() => setOpen(open === "local" ? null : "local")}>
        <div className="choice__title"><I.Shield style={{ width: 20, height: 20, color: "#2cc9b6" }} />Local AI (Ollama)<span className="choice__badge">Private · free</span></div>
        <div className="choice__desc">Runs a model on this computer. Your words never leave it, and there is nothing to pay.</div>
        {choice === "local" && <div className="choice__status">On · {llm.model}</div>}
      </button>
      {open === "local" && <LocalCard ctx={ctx} onDone={() => { setOpen(null); finish(); }} />}

      {/* publik API: hidden in builds without its app token, unless this computer already has a key */}
      {(publik.can_provision || publik.has_key) && (
        <button className={"choice" + (choice === "publik" ? " selected" : "")} onClick={() => { if (publik.has_key && choice !== "publik") turnOnPublik(); else setOpen(open === "publik" ? null : "publik"); }}>
          <div className="choice__title"><I.Sparkle style={{ width: 20, height: 20, color: "#2cc9b6" }} />publik API<span className="choice__badge">No setup</span></div>
          <div className="choice__desc">Already set up. No account and no key to paste. Priced per use from your publik balance, with every charge on your dashboard.</div>
          {publik.has_key && <div className="choice__status">{choice === "publik" ? "On" : "Set up"}{publikState ? ` · ${publikState}` : ""} · {publik.balance_line}{publik.week_label ? ` · ${publik.week_label}` : ""}</div>}
        </button>
      )}
      {open === "publik" && !publik.has_key && (
        <div className="card" style={{ marginTop: -4 }}>
          <div className="card__title" style={{ fontSize: 15 }}>Turn on publik API?</div>
          <p style={{ color: "var(--text-subdued)", fontSize: 14, lineHeight: 1.5, margin: "8px 0 0" }}><b>Cost.</b> {publik.disclosure.cost}</p>
          <p style={{ color: "var(--text-subdued)", fontSize: 14, lineHeight: 1.5, margin: "8px 0 0" }}><b>Where your words go.</b> {publik.disclosure.data}</p>
          {error && <div className="notice">{error}</div>}
          <div className="row-actions">
            <button className="btn btn--primary" disabled={busy || !publik.can_provision} onClick={turnOnPublik}>{busy ? "Setting up…" : "Turn on publik API"}</button>
            <button className="btn btn--ghost" disabled={busy} onClick={() => setOpen(null)}>Not now</button>
          </div>
          <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 10 }}>By continuing you agree to the <a href="#" onClick={(e) => { e.preventDefault(); ipc.openUrl(publik.disclosure.terms_url); }}>publik API terms</a>.</div>
        </div>
      )}
      {publik.has_key && choice === "publik" && (
        <PublikCard ctx={ctx} onDone={finish} />
      )}

      {/* Your own key */}
      <button className={"choice" + (choice === "key" ? " selected" : "")} onClick={() => setOpen(open === "key" ? null : "key")}>
        <div className="choice__title"><I.Lock style={{ width: 20, height: 20 }} />Your own key</div>
        <div className="choice__desc">OpenAI, Anthropic, OpenRouter, Groq, Google or NVIDIA. Paste the key; the provider and model are picked for you.</div>
        {keyConnected && <div className="choice__status">Connected · {llm.model}</div>}
      </button>
      {open === "key" && (
        <div className="card" style={{ marginTop: -4 }}>
          <div className="field"><label>{keyKind(key) || "API key"}</label><input type="password" autoFocus placeholder="sk-…" value={key} onChange={(e) => setKey(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") connectKey(); }} /></div>
          {error && <div className="notice">{error}</div>}
          <div className="row-actions">
            <button className="btn btn--primary" disabled={busy || !key.trim()} onClick={connectKey}>{busy ? "Checking the key…" : "Connect"}</button>
            <button className="btn btn--ghost" disabled={busy} onClick={() => { setOpen(null); setError(null); }}>Cancel</button>
          </div>
          <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 6 }}>The key stays on this computer. GSpot lists your account's models and picks the best fit; change it under Advanced.</div>
        </div>
      )}

      <details className="adv">
        <summary>Get a free key</summary>
        {FREE_KEYS.map((f) => (
          <div key={f.name} style={{ display: "flex", alignItems: "center", gap: 10, padding: "6px 0" }}>
            <div style={{ flex: 1 }}><b>{f.name}</b><div style={{ color: "var(--text-subdued)", fontSize: 13 }}>{f.note}</div></div>
            <button className="btn btn--outline" onClick={() => ipc.openUrl(f.url)}>Get key</button>
          </div>
        ))}
        <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 6 }}>Paste the key into Your own key above. With a cloud key, the text you check with AI features goes to that provider; spelling and grammar stay on this computer.</div>
      </details>

      {llm.provider !== "ollama" && (
        <label className="kv"><span>Use Local AI when the cloud AI hits its limit<br /><span style={{ color: "var(--text-subdued)", fontSize: 13 }}>If your key runs out or is offline, answers come from Ollama on this computer instead.</span></span><input type="checkbox" checked={ctx.settings.ai_fallback_local ?? true} onChange={(e) => ctx.updateSettings({ ai_fallback_local: e.target.checked })} /></label>
      )}

      {mode === "onboarding" && (
        <div className="row-actions" style={{ justifyContent: "center", marginTop: 18 }}>
          <button className="btn btn--ghost" onClick={() => onDone?.()}>Skip for now · spelling and grammar work without AI</button>
        </div>
      )}

      <details className="adv">
        <summary>Advanced</summary>
        <div className="field"><label>Provider</label><div className="select-wrap"><select className="select" value={llm.provider} onChange={(e) => {
          const p = e.target.value as LlmConfig["provider"];
          const base = p === "ollama" ? "http://127.0.0.1:11434" : p === "openai" ? "https://api.openai.com" : p === "anthropic" ? "https://api.anthropic.com" : "https://publikhq.com/api/v1";
          const model = p === "ollama" ? "" : p === "openai" ? "gpt-4o-mini" : p === "anthropic" ? "claude-sonnet-5-5" : "publik-balanced";
          ctx.updateSettings({ llm: { ...llm, provider: p, base_url: base, model } });
        }}>
          <option value="ollama">Ollama (local)</option><option value="publik">publik API</option><option value="openai">OpenAI-compatible</option><option value="anthropic">Anthropic</option>
        </select></div></div>
        <AdvField label="Base URL" value={llm.base_url} onSave={(v) => ctx.updateSettings({ llm: { ...llm, base_url: v } })} />
        <AdvField label="Model" value={llm.model} onSave={(v) => ctx.updateSettings({ llm: { ...llm, model: v } })} />
        {llm.provider !== "publik" && llm.provider !== "ollama" && <AdvField label="API key" value={llm.api_key} secret onSave={(v) => ctx.updateSettings({ llm: { ...llm, api_key: v } })} />}
        <div className="row-actions">
          <button className="btn btn--outline" onClick={async () => { setPing("…"); try { const r = await ipc.runFeature<{ reply: string }>("ping_llm"); setPing(`OK: ${r.reply.slice(0, 40)}`); } catch (e) { setPing(String(e)); } }}>Test connection</button>
          {ping && <span style={{ color: "var(--text-subdued)", fontSize: 13, alignSelf: "center" }}>{ping}</span>}
        </div>
      </details>
    </>
  );
}

type OllamaInfo = { running: boolean; base_url: string; models: string[]; pick: string | null };

/** Local AI: find Ollama, list its models, switch to the chosen one. Without Ollama, say how to get it. */
function LocalCard({ ctx, onDone }: { ctx: Ctx; onDone: () => void }) {
  const [info, setInfo] = useState<OllamaInfo | null>(null);
  const [model, setModel] = useState("");
  const check = async () => {
    setInfo(null);
    const r = await ipc.runFeature<OllamaInfo>("detect_ollama").catch((): OllamaInfo => ({ running: false, base_url: "", models: [], pick: null }));
    setInfo(r);
    setModel(ctx.settings.llm.provider === "ollama" && r.models.includes(ctx.settings.llm.model) ? ctx.settings.llm.model : r.pick ?? "");
  };
  useEffect(() => { check(); }, []);
  const use = async () => {
    await ctx.updateSettings({ llm: { provider: "ollama", base_url: info?.base_url || "http://127.0.0.1:11434", api_key: "", model } });
    ctx.toast(`Local AI on · ${model}`); onDone();
  };
  return (
    <div className="card" style={{ marginTop: -4 }}>
      {!info ? <div style={{ color: "var(--text-subdued)", fontSize: 14 }}>Looking for Ollama…</div>
      : info.running ? (
        <>
          <div className="field"><label>Model</label><div className="select-wrap"><select className="select" value={model} onChange={(e) => setModel(e.target.value)}>
            {info.models.map((m) => <option key={m} value={m}>{m}</option>)}
          </select></div></div>
          <div className="row-actions">
            <button className="btn btn--primary" disabled={!model} onClick={use}>Use {model || "this model"}</button>
            <button className="btn btn--ghost" onClick={check}>Refresh</button>
          </div>
          <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 6 }}>Pick a model that fits in memory: 3–4B (qwen2.5:3b, gemma3:4b) for laptops and 16 GB of RAM or less, 7–8B (qwen2.5:7b) with a GPU or 32 GB.</div>
        </>
      ) : (
        <>
          <p style={{ color: "var(--text-subdued)", fontSize: 14, lineHeight: 1.5, margin: 0 }}>Ollama is not running on this computer. Install it, then pull a model in a terminal:</p>
          <pre style={{ fontSize: 13, margin: "10px 0", padding: "8px 10px", background: "var(--bg-subdued, #2e2e2e)", borderRadius: 6 }}>ollama pull qwen2.5:7b</pre>
          <div className="row-actions">
            <button className="btn btn--primary" onClick={() => ipc.openUrl("https://ollama.com/download")}>Get Ollama</button>
            <button className="btn btn--ghost" onClick={check}>Check again</button>
          </div>
        </>
      )}
    </div>
  );
}

function AdvField({ label, value, secret, onSave }: { label: string; value: string; secret?: boolean; onSave: (v: string) => void }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  return <div className="field"><label>{label}</label><input type={secret ? "password" : "text"} value={v} onChange={(e) => setV(e.target.value)} onBlur={() => { if (v !== value) onSave(v); }} /></div>;
}

/** The publik card under the picker: balance, the one justification, the plan button, a 402 notice with one link. */
export function PublikCard({ ctx, onDone }: { ctx: Ctx; onDone?: () => void }) {
  const p = ctx.publik;
  const [confirm, setConfirm] = useState(false);
  const act = async (f: () => Promise<unknown>) => { try { await f(); ctx.refresh(); } catch (e) { ctx.toast(String(e)); } };
  return (
    <div className="card" style={{ marginTop: -4, borderColor: "#1f5c50", background: "#16302b" }}>
      {p.first_run_owed ? (
        <>
          <div className="card__title" style={{ fontSize: 15 }}>publik API is on</div>
          <div style={{ fontSize: 15, fontWeight: 600, color: "#9ee8d8", marginTop: 8 }}>{p.balance_line}</div>
          <p style={{ color: "var(--text-subdued)", fontSize: 14, lineHeight: 1.5, margin: "8px 0 0" }}>{p.justification}</p>
          <div className="row-actions">
            <button className="btn btn--primary" onClick={() => act(async () => { await ipc.openUrl(p.cta.url); await ipc.publikDismissFirstRun(); onDone?.(); })}>{p.cta.label}</button>
            <button className="btn btn--ghost" onClick={() => act(async () => { await ipc.publikDismissFirstRun(); onDone?.(); })}>{p.later_label}</button>
          </div>
          <div style={{ fontSize: 12, color: "var(--text-muted)", marginTop: 8 }}>"{p.later_label}" keeps publik API on at $0.00; sign in any time from this card in Settings.</div>
        </>
      ) : (
        <>
          <div style={{ fontSize: 14, color: "#9ee8d8" }}>{p.balance_line}{p.last_charge_label ? ` · ${p.last_charge_label}` : ""}</div>
          <p style={{ color: "var(--text-subdued)", fontSize: 13, lineHeight: 1.5, margin: "8px 0 0" }}>{p.justification}</p>
          <div className="row-actions">
            <button className="btn btn--primary" onClick={() => ipc.openUrl(p.cta.url)}>{p.cta.label}</button>
            <button className="btn btn--ghost" onClick={() => ipc.openUrl(p.dashboard_url)}>Dashboard</button>
            {confirm ? (
              <button className="btn btn--ghost" style={{ color: "#ff9a95" }} onClick={() => act(async () => { await ipc.publikForget(); setConfirm(false); })}>Really forget this computer's key</button>
            ) : <button className="btn btn--ghost" onClick={() => setConfirm(true)}>Turn off</button>}
          </div>
        </>
      )}
      {p.notice && (
        <div className="notice" style={{ marginBottom: 0 }}>{p.notice.message}{p.notice.link_url && <> <a href="#" onClick={(e) => { e.preventDefault(); ipc.openUrl(p.notice!.link_url); }}>{p.notice.link_label || "Open publik"}</a></>}</div>
      )}
    </div>
  );
}
