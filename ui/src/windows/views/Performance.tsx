import { useEffect, useState } from "react";
import type { Ctx } from "../Panel";
import { ipc } from "../../lib/ipc";
import type { ParagraphScore } from "../../lib/types";
import * as I from "../../components/icons";
import { NoDoc, TitleBar } from "../Panel";

// Pill colors by reading ease: green easy, amber fairly hard, red hard.
function pillColor(p: ParagraphScore): string {
  if (p.flesch >= 60) return "#2cc9b6";
  if (p.flesch >= 50) return "#ffbf47";
  return "#ff7a74";
}

export function Performance({ ctx }: { ctx: Ctx }) {
  const [paras, setParas] = useState<ParagraphScore[]>([]);
  useEffect(() => {
    if (!ctx.doc) return;
    ipc.runFeature<{ paragraphs: ParagraphScore[] }>("readability").then((r) => setParas(r.paragraphs ?? [])).catch(() => setParas([]));
  }, [ctx.doc?.version]);
  if (!ctx.doc) return <NoDoc ctx={ctx} />;
  const hardest = paras.length > 1 ? paras.reduce((a, b) => (b.grade > a.grade ? b : a)) : paras[0];
  const s = ctx.score;
  const rows = s ? [
    { label: "Correctness", color: "#ff7a74", fill: s.correctness_fill, val: s.correctness_label, ok: s.correctness_count === 0 },
    { label: "Clarity", color: "#7d99f0", fill: s.clarity_fill, val: s.clarity_label, ok: s.clarity_count === 0 },
    { label: "Engagement", color: "#2cc9b6", fill: s.engagement_fill, val: s.engagement_label, ok: s.engagement_count === 0 },
    { label: "Delivery", color: "#a598f0", fill: s.delivery_fill, val: s.delivery_label, ok: s.delivery_count === 0 },
  ] : [];
  return (
    <>
      <TitleBar title="Performance" />
      <div className="panel__scroll">
        <div className="score-head">
          <div className="score-big"><b>{s?.overall ?? "–"}</b><span>of 100</span></div>
          <div>
            <h3>Overall score</h3>
            <p>{s?.note ?? "The score represents the quality of this text. Increase it by addressing issues."}</p>
          </div>
        </div>
        {rows.map((r) => (
          <div className="score-row" key={r.label}>
            <div className="lbl"><i style={{ width: 12, height: 12, borderRadius: "50%", background: r.color, display: "inline-block" }} />{r.label}</div>
            <div className="bar"><i style={{ width: `${r.fill}%`, background: r.color }} /></div>
            <div className="val">{r.ok ? <I.Check style={{ width: 16, height: 16, color: "#2cc9b6" }} /> : null}{r.val}</div>
          </div>
        ))}
        {paras.length > 0 && (
          <div style={{ marginTop: 24 }}>
            <div className="section-label">Paragraphs</div>
            {paras.map((p) => (
              <div className="kv" key={p.index} style={{ gap: 10 }}>
                <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{p.preview}…</span>
                <span style={{ color: "#0e1f1c", background: pillColor(p), borderRadius: 10, padding: "2px 10px", fontSize: 12, fontWeight: 600, whiteSpace: "nowrap" }}>{p.label}</span>
                <span style={{ color: "var(--text-subdued)", fontSize: 13, whiteSpace: "nowrap" }}>grade {p.grade.toFixed(1)}</span>
              </div>
            ))}
            {hardest && hardest.flesch < 60 && <p style={{ color: "var(--text-subdued)", fontSize: 13, margin: "8px 0 0" }}>Hardest: "{hardest.preview}…" Split long sentences or use shorter words.</p>}
          </div>
        )}
        {s && (
          <div style={{ marginTop: 24 }}>
            <div className="section-label">Text statistics</div>
            <div className="kv"><span>Words</span><span>{s.words}</span></div>
            <div className="kv"><span>Characters</span><span>{s.characters}</span></div>
            <div className="kv"><span>Sentences</span><span>{s.sentences}</span></div>
            <div className="kv"><span>Reading time</span><span>{Math.max(1, Math.round(s.reading_time_sec / 60))} min</span></div>
            <div className="kv"><span>Readability (Flesch)</span><span>{Math.round(s.flesch)}</span></div>
          </div>
        )}
      </div>
    </>
  );
}
