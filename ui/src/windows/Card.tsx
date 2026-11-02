import { useEffect, useRef, useState } from "react";
import { ipc, on } from "../lib/ipc";
import { wordDiff } from "../lib/text";
import type { Alert } from "../lib/types";

/** The hover card: its own window, positioned by the host next to the underlined word. */
export function Card() {
  const [alerts, setAlerts] = useState<Record<string, Alert>>({});
  const [open, setOpen] = useState<string | null>(null);
  const [suggesting, setSuggesting] = useState<string | null>(null);
  const cardRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    ipc.getState().then((s) => setAlerts(Object.fromEntries(s.alerts.map((a) => [a.id, a])))).catch(() => {});
    const u1 = on<Alert[]>("host://alerts", (a) => setAlerts(Object.fromEntries(a.map((x) => [x.id, x]))));
    const u2 = on<{ id: string }>("card://show", (p) => setOpen(p.id));
    const u3 = on("card://hide", () => setOpen(null));
    return () => { u1.then((f) => f()); u2.then((f) => f()); u3.then((f) => f()); };
  }, []);

  const alert = open ? alerts[open] : undefined;

  // Tell the host how big the card is, so its window fits and stays on screen.
  useEffect(() => {
    const el = cardRef.current;
    if (!el) return;
    const report = () => {
      const r = el.getBoundingClientRect();
      ipc.setCardSize(Math.ceil(r.width) + 2, Math.ceil(r.height) + 2).catch(() => {});
    };
    report();
    const ro = new ResizeObserver(report);
    ro.observe(el);
    return () => ro.disconnect();
  }, [alert?.id]);

  // After any action the card closes.
  const done = () => { setOpen(null); ipc.hideCard().catch(() => {}); };

  if (!alert) return null;
  return (
    <div ref={cardRef} className="ul-card">
      <div style={{ display: "flex", alignItems: "center", gap: 8, color: "#bcbcbc", fontSize: 13 }}>
        <i style={{ width: 10, height: 10, borderRadius: 5, background: `var(--${alert.category})`, display: "inline-block" }} />
        {cap(alert.category)} · {alert.title}
      </div>
      <div style={{ fontSize: 17, margin: "10px 0 12px", maxHeight: 220, overflowY: "auto" }}>
        {alert.suggestions.length === 0 ? (
          <span style={{ color: "#e6e6e6" }}>{alert.original}</span>
        ) : (
          alert.suggestions[0] !== "" && alert.original.split(/\s+/).length > 6 ? (
            <span className="diff">
              {wordDiff(alert.original, alert.suggestions[0]!).map((op, i) => op.kind === "same" ? <span key={i}>{op.text}</span> : op.kind === "del" ? <del key={i}>{op.text}</del> : <ins key={i}>{op.text}</ins>)}
            </span>
          ) : (
            <>
              <del style={{ color: "#9a9a9a", marginRight: 6 }}>{alert.original}</del>
              <ins style={{ textDecoration: "none", fontWeight: 600 }}>{alert.suggestions[0] === "" ? "Remove" : alert.suggestions[0]}</ins>
            </>
          )
        )}
      </div>
      <div style={{ fontSize: 13, color: "#bcbcbc", marginBottom: 12 }}>{alert.message}</div>
      <div style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap" }}>
        {alert.suggestions.length > 0 && (
          <button className="btn btn--primary" onClick={() => { ipc.acceptAlert(alert.id, 0).catch(() => {}); done(); }}>Accept</button>
        )}
        {alert.suggestions.length > 0 && <span style={{ fontSize: 12, color: "#8c8c8c" }}>Ctrl+Shift+Y</span>}
        {alert.suggestions.length === 0 && (
          <button className="btn btn--outline" disabled={suggesting === alert.id} onClick={() => { setSuggesting(alert.id); ipc.runFeature("word_options", { id: alert.id }).catch(() => {}).finally(() => setSuggesting(null)); }}>
            {suggesting === alert.id ? "Suggesting…" : "Suggest options"}
          </button>
        )}
        <button className="btn btn--ghost" onClick={() => { ipc.dismissAlert(alert.id).catch(() => {}); done(); }}>Dismiss</button>
        {(alert.kind === "Spelling" || alert.kind === "Typo") && (
          <button className="btn btn--ghost" onClick={() => { ipc.addUserWord(alert.original.trim()).catch(() => {}); done(); }}>Add to dictionary</button>
        )}
      </div>
      {alert.suggestions.length > 1 && (
        <div style={{ display: "flex", gap: 10, flexWrap: "wrap", marginTop: 10 }}>
          {alert.suggestions.slice(1, 4).map((s, i) => <button key={i} className="btn btn--outline" onClick={() => { ipc.acceptAlert(alert.id, i + 1).catch(() => {}); done(); }}>{s || "Remove"}</button>)}
        </div>
      )}
    </div>
  );
}

function cap(s: string) { return s.charAt(0).toUpperCase() + s.slice(1); }
