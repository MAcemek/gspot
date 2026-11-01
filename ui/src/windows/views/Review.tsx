import { useEffect, useMemo, useState } from "react";
import type { Ctx } from "../Panel";
import { NoDoc, Empty } from "../Panel";
import { ipc } from "../../lib/ipc";
import { wordDiff } from "../../lib/text";
import type { Alert, Category, SpotResult } from "../../lib/types";
import * as I from "../../components/icons";

const CATS: { id: Category; label: string; color: string }[] = [
  { id: "correctness", label: "Correctness", color: "#ff7a74" },
  { id: "clarity", label: "Clarity", color: "#7d99f0" },
  { id: "engagement", label: "Engagement", color: "#2cc9b6" },
  { id: "delivery", label: "Delivery", color: "#a598f0" },
];
const CAT_IDS = new Set<string>(CATS.map((c) => c.id));

function initialTab(): Category {
  try {
    const t = new URLSearchParams(window.location.search).get("tab");
    if (t && CAT_IDS.has(t)) return t as Category;
  } catch { /* no URL in some webviews */ }
  return "correctness";
}

/** A long rewrite reads better as a word-level diff than as two long runs of text. */
export function Diff({ from, to }: { from: string; to: string }) {
  const ops = wordDiff(from, to);
  return (
    <span className="diff">
      {ops.map((op, i) => op.kind === "same" ? <span key={i}>{op.text}</span> : op.kind === "del" ? <del key={i}>{op.text}</del> : <ins key={i}>{op.text}</ins>)}
    </span>
  );
}

/** What a suggestion does: swap text, delete text, or only point something out. */
export function alertMode(a: Alert): "replace" | "remove" | "note" {
  if (a.suggestions.length === 0) return "note";
  return a.suggestions[0] === "" ? "remove" : "replace";
}

export function Review({ ctx }: { ctx: Ctx }) {
  const [tab, setTab] = useState<Category>(initialTab);
  const [open, setOpen] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [suggesting, setSuggesting] = useState<string | null>(null);
  const counts = useMemo(() => Object.fromEntries(CATS.map((c) => [c.id, ctx.alerts.filter((a) => a.category === c.id).length])) as Record<Category, number>, [ctx.alerts]);
  const list = ctx.alerts.filter((a) => a.category === tab);
  useEffect(() => { if (open && !ctx.alerts.some((a) => a.id === open)) setOpen(list[0]?.id ?? null); }, [ctx.alerts]);
  useEffect(() => { if (!open && list.length) setOpen(list[0]!.id); }, [tab, list.length]);

  // Spot check on demand. The host runs it by itself once typing pauses
  // (Settings → Suggestions), so this button is for "check it now".
  const [spotBusy, setSpotBusy] = useState(false);
  const [spotNote, setSpotNote] = useState<string | null>(null);
  const runSpot = async () => {
    if (spotBusy || ctx.spot_running || !ctx.doc) return;
    const words = ctx.doc.text.split(/\s+/).filter(Boolean).length;
    if (words < 8) { ctx.toast("Write a few sentences first."); return; }
    setSpotBusy(true); setSpotNote(null);
    try {
      const r = await ipc.runFeature<SpotResult>("spot_check");
      if (r.busy) { setSpotNote("A check is already running."); return; }
      if (r.stale) return;
      const n = r.suggestions;
      setSpotNote(n === 0 ? `Checked ${r.checked + r.from_cache} sentences. Nothing to change.` : `${n} sentence suggestion${n === 1 ? "" : "s"}.`);
      if (n > 0 && tab !== "clarity") setTab("clarity");
    } catch (e) { ctx.toast(String(e)); setSpotNote(null); }
    finally { setSpotBusy(false); }
  };
  useEffect(() => { if (ctx.spot_running) setSpotNote(null); }, [ctx.spot_running]);

  if (!ctx.doc) return <NoDoc ctx={ctx} />;

  const accept = async (a: Alert, idx = 0) => {
    setBusy(a.id);
    try { const r = await ipc.acceptAlert(a.id, idx); ctx.toast(r.method === "ax" ? "Applied" : "Applied via paste"); }
    catch (e) { ctx.toast(String(e)); }
    finally { setBusy(null); }
  };
  const suggest = async (a: Alert) => {
    setSuggesting(a.id);
    try { await ipc.runFeature("word_options", { id: a.id }); }
    catch (e) { ctx.toast(String(e)); }
    finally { setSuggesting(null); }
  };
  const dismiss = async (a: Alert) => { try { await ipc.dismissAlert(a.id); } catch (e) { ctx.toast(String(e)); } };
  const skipApp = async () => {
    const d = ctx.doc!;
    if (!d.bundle_id || ctx.settings.skip_apps.some((a) => a.bundle_id === d.bundle_id)) return;
    try {
      await ctx.updateSettings({ skip_apps: [...ctx.settings.skip_apps, { bundle_id: d.bundle_id, name: d.app_name }] });
      ctx.toast(`GSpot is off in ${d.app_name}. Settings → Skipped apps turns it back on.`);
    } catch (e) { ctx.toast(String(e)); }
  };
  const running = spotBusy || ctx.spot_running;

  return (
    <>
      <div className="panel__scroll" style={{ paddingTop: 0 }}>
        <div className="review-head">Review suggestions <span className="count-pill">{ctx.alerts.length}</span>
          <button className="btn btn--outline spot-btn" title="Check every sentence for wordiness, tangles, long run-ons and repeats" disabled={running} onClick={runSpot}>
            {running ? <div className="spin" style={{ width: 14, height: 14, borderWidth: 2 }} /> : <I.Sparkle style={{ width: 16, height: 16, color: "#7d99f0" }} />}{running ? "Checking" : "Spot check"}
          </button>
        </div>
        <div className="review-app"><span>{ctx.doc.app_name || "This app"}</span><button className="linkbtn" title="GSpot stays out of this app until you turn it back on in Settings" onClick={skipApp}>Skip this app</button></div>
        {(spotNote || running) && <div className="review-note">{running ? "Checking your sentences with the model." : spotNote}</div>}
        <div className="cats">
          {CATS.map((c) => {
            const n = counts[c.id];
            const fill = ctx.score ? (ctx.score as any)[`${c.id}_fill`] as number : 100;
            return (
              <button key={c.id} className={"cat" + (tab === c.id ? " active" : "")} onClick={() => setTab(c.id)} style={{ "--cat": c.color } as React.CSSProperties} title={`${c.label}: ${n} suggestion${n === 1 ? "" : "s"}`}>
                <span className="cat__bar"><i style={{ width: `${Math.max(6, Math.min(100, fill))}%` }} /></span>
                <span className="cat__label">{c.label}</span>
                <span className={"cat__count" + (n === 0 ? " zero" : "")}>{n === 0 ? <I.Check style={{ width: 12, height: 12 }} /> : n}</span>
              </button>
            );
          })}
        </div>
        {list.length === 0 ? (
          <Empty icon={tab === "correctness" ? <I.Check /> : <I.Sparkle />} title={tab === "correctness" ? "Looking good" : `No ${tab} suggestions`}>
            {ctx.doc.text.trim().length === 0 ? "Start typing and suggestions appear here." : "Nothing to fix in this category right now."}
          </Empty>
        ) : (
          list.map((a) => {
            const c = CATS.find((x) => x.id === a.category)!;
            const isOpen = open === a.id;
            const mode = alertMode(a);
            return isOpen ? (
              <div key={a.id} className="alert alert--open">
                <div className="alert__kind"><span className="alert__dot"><i style={{ background: c.color }} /></span>{c.label} · <b>{a.title}</b><button title="Why?" style={{ marginLeft: "auto", color: "#bcbcbc" }} onClick={() => ctx.toast(a.message)}><I.Info style={{ width: 18, height: 18 }} /></button></div>
                <div className="alert__preview">
                  {mode === "note" ? (
                    <span className="alert__quote">{a.original}</span>
                  ) : (
                    <>
                      {mode === "replace" && a.original.split(/\s+/).length > 6 ? (
                        <Diff from={a.original} to={a.suggestions[0]!} />
                      ) : (
                        <>
                          <del>{a.original}</del>
                          {mode === "remove" ? <span className="alert__hint">Remove</span> : <ins>{a.suggestions[0]}</ins>}
                        </>
                      )}
                      <span className="alert__ctx"> {context(ctx.doc!.text, a)}</span>
                    </>
                  )}
                </div>
                <div className="alert__why">{a.message}{mode === "note" && a.category === "clarity" && running ? " Checking for a rewrite with the model." : ""}</div>
                <div className="alert__actions">
                  {mode !== "note" && (
                    <button className="btn btn--primary" disabled={busy === a.id} onClick={() => accept(a, 0)}>{busy === a.id ? "Applying" : "Accept"}</button>
                  )}
                  {mode === "note" && (
                    <button className="btn btn--outline" disabled={suggesting === a.id} onClick={() => suggest(a)}>
                      {suggesting === a.id ? <div className="spin" style={{ width: 14, height: 14, borderWidth: 2 }} /> : <I.Sparkle style={{ width: 16, height: 16, color: "#7d99f0" }} />}{suggesting === a.id ? "Suggesting…" : "Suggest options"}
                    </button>
                  )}
                  <button className="btn btn--ghost" onClick={() => dismiss(a)}>Dismiss</button>
                  {a.suggestions.length > 1 && (
                    <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
                      {a.suggestions.slice(1, 4).map((s, i) => <button key={i} className="btn btn--outline" style={{ padding: "6px 12px" }} onClick={() => accept(a, i + 1)}>{s || "Remove"}</button>)}
                    </div>
                  )}
                </div>
              </div>
            ) : (
              <div key={a.id} className="alert" onClick={() => setOpen(a.id)} style={{ cursor: "pointer" }}>
                <span className="alert__dot"><i style={{ background: c.color }} /></span>
                <div className="alert__body"><div className="alert__kind">{a.title}</div><div className="alert__text">{a.original.trim() || "Space"}</div></div>
              </div>
            );
          })
        )}
      </div>
    </>
  );
}

function context(text: string, a: Alert): string {
  const after = text.slice(a.u16_end, a.u16_end + 24).replace(/\s+/g, " ");
  return after.length ? after + "…" : "";
}
