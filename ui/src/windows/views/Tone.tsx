import { useEffect } from "react";
import type { Ctx } from "../Panel";
import { NoDoc, TitleBar, Empty, useFeature } from "../Panel";
import * as I from "../../components/icons";

const GOALS: { id: string; label: string }[] = [
  { id: "", label: "None" }, { id: "formal", label: "Formal" }, { id: "friendly", label: "Friendly" },
  { id: "confident", label: "Confident" }, { id: "concise", label: "Concise" },
];

/** Chips for the writing goal the sentence spot check steers toward. */
export function GoalPicker({ ctx }: { ctx: Ctx }) {
  const cur = ctx.settings.writing_goal ?? "";
  return (
    <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
      {GOALS.map((g) => (
        <button key={g.id} className="btn btn--outline" aria-pressed={cur === g.id}
          style={{ padding: "5px 12px", ...(cur === g.id ? { background: "var(--bg-subdued)", borderColor: "var(--engagement)" } : {}) }}
          onClick={() => ctx.updateSettings({ writing_goal: g.id }).catch((e) => ctx.toast(String(e)))}>{g.label}</button>
      ))}
    </div>
  );
}

interface ToneResult { too_short?: boolean; tones: { name: string; confidence: number }[]; summary: string }

export function Tone({ ctx }: { ctx: Ctx }) {
  const f = useFeature<ToneResult>("tone", ["Reading your text", "Detecting tone"]);
  useEffect(() => { if (ctx.doc && ctx.doc.text.trim().split(/\s+/).length >= 12) f.run(); }, [ctx.doc?.version]);
  if (!ctx.doc) return <NoDoc ctx={ctx} />;
  const d = f.data;
  return (
    <>
      <TitleBar title="Tone insights" onReload={() => f.run()} />
      <div className="panel__scroll">
        <div className="section-label">Goal</div>
        <GoalPicker ctx={ctx} />
        <p style={{ color: "var(--text-subdued)", fontSize: 13, margin: "8px 0 16px" }}>GSpot rephrases sentences that clearly miss your goal.</p>
        {f.loading && !d ? <div className="empty"><div className="spin" /><p style={{ marginTop: 16 }}>Detecting your tone.</p></div>
        : f.error ? <div className="notice">{f.error}</div>
        : !d || d.too_short ? (
          <Empty icon={<I.ToneWave />} title="We couldn't detect your tone yet">Add more text to get tone insights.</Empty>
        ) : (
          <>
            <div className="tone-list">
              {d.tones.map((t, i) => (
                <div className="tone" key={t.name}><span className="dot" style={{ opacity: 1 - i * 0.3 }} />{t.name}<span className="pct">{Math.round(t.confidence * 100)}%</span></div>
              ))}
            </div>
            <p style={{ color: "var(--text-subdued)", fontSize: 15, lineHeight: 1.5, marginTop: 18 }}>{d.summary}</p>
          </>
        )}
      </div>
    </>
  );
}
