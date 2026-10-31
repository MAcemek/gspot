import { useState } from "react";
import type { Ctx } from "../Panel";
import { Hero, NoDoc, Steps, TitleBar, useFeature, Empty } from "../Panel";
import { ipc } from "../../lib/ipc";
import { AUDIENCES, PARAPHRASE_STYLES, VOICES, wordDiff } from "../../lib/text";
import * as I from "../../components/icons";

function Diff({ a, b }: { a: string; b: string }) {
  return <div className="diff">{wordDiff(a, b).map((o, i) => o.kind === "same" ? <span key={i}>{o.text}</span> : o.kind === "del" ? <del key={i}>{o.text}</del> : <ins key={i}>{o.text}</ins>)}</div>;
}

export function RewriteResult({ ctx, original, rewritten, usedSelection, onBack }: { ctx: Ctx; original: string; rewritten: string; usedSelection: boolean; onBack: () => void }) {
  const [showDiff, setShowDiff] = useState(false);
  const apply = async () => {
    const d = ctx.doc; if (!d) return;
    const [s, e] = usedSelection ? [d.sel_start, d.sel_end] : [0, d.text.length];
    try { const r = await ipc.replaceRange(s, e, rewritten); ctx.toast(r.method === "ax" ? "Replaced" : "Replaced via paste"); }
    catch (err) { ctx.toast(String(err)); }
  };
  return (
    <>
      <div className="result-box">{showDiff ? <Diff a={original} b={rewritten} /> : rewritten}</div>
      <div className="row-actions">
        <button className="btn btn--primary" onClick={apply}>Replace {usedSelection ? "selection" : "text"}</button>
        <button className="btn btn--outline" onClick={() => navigator.clipboard.writeText(rewritten).then(() => ctx.toast("Copied"))}>Copy</button>
        <button className="btn btn--ghost" onClick={() => setShowDiff((v) => !v)}>{showDiff ? "Hide changes" : "Show changes"}</button>
        <button className="btn btn--ghost" onClick={onBack}>Back</button>
      </div>
    </>
  );
}

export function Paraphraser({ ctx }: { ctx: Ctx }) {
  const f = useFeature<{ rewritten: string; summary: string; style: string; used_selection: boolean }>("paraphrase", ["Reading your text", "Rewriting", "Polishing"]);
  const [style, setStyle] = useState<string | null>(null);
  if (!ctx.doc) return <NoDoc ctx={ctx} />;
  const src = ctx.doc.selected_text || ctx.doc.text;
  return (
    <>
      <TitleBar title="" onReload={() => style && f.run({ style })} />
      <div className="panel__scroll">
        <Hero icon={<I.Paraphrase />} title="Paraphraser">
          <p>{f.data?.summary ?? (ctx.doc.selected_text ? "Rewrite the selected text in a new style." : "Rewrite your text in a new style, or select a passage to rewrite just that.")}</p>
        </Hero>
        {f.loading ? <Steps steps={["Reading your text", "Rewriting", "Polishing"]} active={f.step} />
        : f.error ? <div className="notice">{f.error}</div>
        : f.data ? <RewriteResult ctx={ctx} original={src} rewritten={f.data.rewritten} usedSelection={f.data.used_selection} onBack={f.reset} />
        : (
          <>
            <div className="section-label">Change writing style</div>
            {PARAPHRASE_STYLES.map((s) => (
              <button key={s.id} className={"list-opt" + (style === s.id ? " selected" : "")} style={{ width: "100%", textAlign: "left" }} onClick={() => { setStyle(s.id); f.run({ style: s.id }); }}>{s.id}</button>
            ))}
          </>
        )}
      </div>
    </>
  );
}

export function Humanizer({ ctx }: { ctx: Ctx }) {
  const f = useFeature<{ rewritten: string; changes: string[]; voice: string; used_selection: boolean }>("humanize", ["Analyzing your document", "Adopting the voice", "Humanizing"]);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [sample, setSample] = useState("");
  const [saving, setSaving] = useState(false);
  if (!ctx.doc) return <NoDoc ctx={ctx} />;
  const src = ctx.doc.selected_text || ctx.doc.text;
  const glyph = (g: string) => g === "face" ? <I.Face /> : g === "scissors" ? <I.Scissors /> : g === "cup" ? <I.Cup /> : g === "compass" ? <I.Compass /> : <I.Quill />;
  const createVoice = async () => {
    setSaving(true);
    try { await ipc.runFeature("create_voice", { name, sample: sample || src }); ctx.refresh(); setCreating(false); setName(""); setSample(""); ctx.toast("Voice created"); }
    catch (e) { ctx.toast(String(e)); } finally { setSaving(false); }
  };
  return (
    <>
      <div className="titlebar" style={{ justifyContent: "flex-start", paddingLeft: 16, gap: 10, fontSize: 17 }}><I.Heart style={{ width: 22, height: 22 }} /> Humanizer
        <div className="titlebar__actions"><button title="Reload" onClick={f.reset}><I.Refresh /></button></div>
      </div>
      <div className="panel__scroll">
        {f.loading ? <div className="empty"><div className="spin" /><p style={{ marginTop: 16 }}>{["Analyzing your document", "Adopting the voice", "Humanizing"][Math.min(f.step, 2)]}</p></div>
        : f.error ? <div className="notice">{f.error}</div>
        : f.data ? (
          <>
            <div className="section-label">Rewritten in {f.data.voice}</div>
            <RewriteResult ctx={ctx} original={src} rewritten={f.data.rewritten} usedSelection={f.data.used_selection} onBack={f.reset} />
            {f.data.changes?.length ? <ul style={{ color: "var(--text-subdued)", fontSize: 14, paddingLeft: 18, marginTop: 14 }}>{f.data.changes.map((c, i) => <li key={i}>{c}</li>)}</ul> : null}
          </>
        ) : creating ? (
          <>
            <div className="section-label">Create a voice</div>
            <div className="field"><label>Name</label><input value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. The Storyteller" /></div>
            <div className="field"><label>Writing sample (leave empty to use your current text)</label><textarea value={sample} onChange={(e) => setSample(e.target.value)} placeholder="Paste something you wrote…" /></div>
            <div className="row-actions"><button className="btn btn--primary" disabled={saving} onClick={createVoice}>{saving ? "Creating your voice…" : "Save voice"}</button><button className="btn btn--ghost" onClick={() => setCreating(false)}>Cancel</button></div>
          </>
        ) : (
          <>
            <button className="card" style={{ width: "100%", textAlign: "left", color: "#8fb3ff", fontSize: 16, display: "flex", alignItems: "center", gap: 10, padding: "18px 16px" }} onClick={() => setCreating(true)}><I.Plus style={{ width: 20, height: 20 }} /> Create a voice</button>
            <div style={{ height: 24 }} />
            {ctx.settings.custom_voices.map((v) => (
              <button key={v.id} className="card card--row" style={{ width: "100%", textAlign: "left" }} onClick={() => f.run({ voice_name: v.name, voice_description: v.description })}>
                <span className="card__glyph"><I.Pencil /></span><span><div className="card__title">{v.name}</div><div className="card__desc">{v.description.slice(0, 80)}{v.description.length > 80 ? "…" : ""}</div></span>
              </button>
            ))}
            {VOICES.map((v) => (
              <button key={v.id} className="card" style={{ width: "100%", textAlign: "left" }} onClick={() => f.run({ voice_name: v.name, voice_description: v.guide })}>
                {v.isNew && <span className="chip-new">New</span>}
                <div className="card--row"><span className="card__glyph">{glyph(v.glyph)}</span><span><div className="card__title">{v.name}</div><div className="card__desc">{v.desc}</div></span></div>
              </button>
            ))}
          </>
        )}
      </div>
    </>
  );
}

const CEO_LEVELS = [
  { id: "polished", name: "Polished", desc: "Short and direct, still clean.", glyph: "scissors" },
  { id: "casual", name: "Casual", desc: "Drops the last period, uses & and w/.", glyph: "cup" },
  { id: "phone", name: "Sent from my phone", desc: "lower case, no apostrophes, thx & pls.", glyph: "bolt" },
];

export function CeoMode({ ctx }: { ctx: Ctx }) {
  const f = useFeature<{ rewritten: string; cut: string[]; level: string; words_in: number; words_out: number; used_selection: boolean }>("ceo", ["Finding the ask", "Cutting the fluff", "Typing it like a CEO"]);
  if (!ctx.doc) return <NoDoc ctx={ctx} />;
  const src = ctx.doc.selected_text || ctx.doc.text;
  const glyph = (g: string) => g === "scissors" ? <I.Scissors /> : g === "cup" ? <I.Cup /> : <I.Bolt />;
  const level = CEO_LEVELS.find((l) => l.id === f.data?.level);
  return (
    <>
      <div className="titlebar" style={{ justifyContent: "flex-start", paddingLeft: 16, gap: 10, fontSize: 17 }}><I.Briefcase style={{ width: 22, height: 22 }} /> Talk like a CEO
        <div className="titlebar__actions"><button title="Reload" onClick={f.reset}><I.Refresh /></button></div>
      </div>
      <div className="panel__scroll">
        {f.loading ? <Steps steps={["Finding the ask", "Cutting the fluff", "Typing it like a CEO"]} active={f.step} />
        : f.error ? <div className="notice">{f.error}</div>
        : f.data ? (
          <>
            <div className="section-label">{level?.name ?? "CEO"} · {f.data.words_in} → {f.data.words_out} words</div>
            <RewriteResult ctx={ctx} original={src} rewritten={f.data.rewritten} usedSelection={f.data.used_selection} onBack={f.reset} />
            {f.data.cut?.length ? <ul style={{ color: "var(--text-subdued)", fontSize: 14, paddingLeft: 18, marginTop: 14 }}>{f.data.cut.map((c, i) => <li key={i}>{c}</li>)}</ul> : null}
          </>
        ) : (
          <>
            <Hero icon={<I.Briefcase />} title="Talk like a CEO">
              <p>{ctx.doc.selected_text ? "Cut the selection down to the ask, typed the way a busy exec would." : "Cut your message down to the ask, typed the way a busy exec would. Select a passage to rewrite just that."}</p>
            </Hero>
            <div className="section-label">How rough?</div>
            {CEO_LEVELS.map((l) => (
              <button key={l.id} className="card card--row" style={{ width: "100%", textAlign: "left" }} onClick={() => f.run({ level: l.id })}>
                <span className="card__glyph">{glyph(l.glyph)}</span><span><div className="card__title">{l.name}</div><div className="card__desc">{l.desc}</div></span>
              </button>
            ))}
          </>
        )}
      </div>
    </>
  );
}

export function Reactions({ ctx }: { ctx: Ctx }) {
  const f = useFeature<{ too_short?: boolean; summary: string; remember: string[]; ask: { label: string; question: string }[]; audience: string }>("reactions", ["Reading as your audience", "Predicting reactions", "Writing questions"]);
  const [picking, setPicking] = useState(false);
  const [creating, setCreating] = useState(false);
  const [aud, setAud] = useState(AUDIENCES[0]!);
  const [cn, setCn] = useState(""); const [cd, setCd] = useState("");
  if (!ctx.doc) return <NoDoc ctx={ctx} />;
  const all = [...ctx.settings.custom_audiences.map((a) => ({ id: a.id, name: a.name, desc: a.description })), ...AUDIENCES];
  const run = (a = aud) => f.run({ audience: a.name, audience_desc: a.desc });
  return (
    <>
      <TitleBar title="" onReload={() => run()} />
      <div className="panel__scroll">
        <Hero icon={<I.Book />} title="Reader Reactions" />
        <button className="audience" style={{ width: "100%" }} onClick={() => setPicking((v) => !v)}><span style={{ display: "flex", alignItems: "center", gap: 10, color: "var(--text-base)" }}><I.Users style={{ width: 20, height: 20, color: "#bcbcbc" }} /><span style={{ color: "#bcbcbc" }}>{aud.name}</span></span><I.ChevronRight style={{ width: 18, height: 18 }} /></button>
        {picking && (
          <div style={{ marginBottom: 16 }}>
            <div className="section-label">Audience · Tailor the feedback to who you're writing for</div>
            {all.map((a) => <button key={a.id} className={"list-opt" + (a.id === aud.id ? " selected" : "")} style={{ width: "100%", textAlign: "left" }} onClick={() => { setAud(a); setPicking(false); run(a); }}>{a.name}<div style={{ color: "var(--text-subdued)", fontSize: 13 }}>{a.desc}</div></button>)}
            {creating ? (
              <>
                <div className="field"><label>Audience name</label><input value={cn} onChange={(e) => setCn(e.target.value)} /></div>
                <div className="field"><label>Describe them</label><textarea value={cd} onChange={(e) => setCd(e.target.value)} /></div>
                <div className="row-actions"><button className="btn btn--primary" onClick={async () => { await ipc.runFeature("create_audience", { name: cn, description: cd }); ctx.refresh(); setCreating(false); }}>Save and recheck</button><button className="btn btn--ghost" onClick={() => setCreating(false)}>Cancel</button></div>
              </>
            ) : <button className="btn btn--outline" onClick={() => setCreating(true)}><I.Plus style={{ width: 18, height: 18 }} /> Create audience</button>}
          </div>
        )}
        {f.loading ? <Steps steps={["Reading as your audience", "Predicting reactions", "Writing questions"]} active={f.step} />
        : f.error ? <div className="notice">{f.error}</div>
        : f.data?.too_short ? <Empty icon={<I.Pencil />} title="Not enough content to review">Text must be a minimum of 25 words to receive feedback.</Empty>
        : f.data ? (
          <>
            <p style={{ fontSize: 17, lineHeight: 1.5, margin: "4px 0 18px" }}>{f.data.summary}</p>
            <div style={{ fontSize: 20, fontWeight: 600, margin: "8px 0 10px" }}>{aud.name} will remember</div>
            {f.data.remember.map((r, i) => <div className="li" key={i}><span className="num">{i + 1}</span><span>{r}</span></div>)}
            <div style={{ fontSize: 20, fontWeight: 600, margin: "22px 0 10px" }}>{aud.name} might ask</div>
            {f.data.ask.map((q, i) => <div className="li" key={i}><span className="num num--q">?</span><span><b>{q.label}</b> · {q.question}</span></div>)}
          </>
        ) : (
          <div className="row-actions"><button className="btn btn--primary" onClick={() => run()}>Predict reactions</button></div>
        )}
      </div>
    </>
  );
}
