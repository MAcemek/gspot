import { useEffect, useRef, useState } from "react";
import { ipc, on } from "../lib/ipc";
import { wordDiff } from "../lib/text";
import type { Alert, Geometry } from "../lib/types";

const CARD_W = 300;

export function Underlines() {
  const [geo, setGeo] = useState<Geometry>({ origin_x: 0, origin_y: 0, width: 0, height: 0, rects: [], hovered: null, over_card: false });
  const [alerts, setAlerts] = useState<Record<string, Alert>>({});
  const [open, setOpen] = useState<string | null>(null);
  const [suggesting, setSuggesting] = useState<string | null>(null);
  const closeTimer = useRef<number | null>(null);
  const cardRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    ipc.getState().then((s) => setAlerts(Object.fromEntries(s.alerts.map((a) => [a.id, a])))).catch(() => {});
    const u1 = on<Geometry>("host://geometry", setGeo);
    const u2 = on<Alert[]>("host://alerts", (a) => setAlerts(Object.fromEntries(a.map((x) => [x.id, x]))));
    return () => { u1.then((f) => f()); u2.then((f) => f()); };
  }, []);

  // The host hit-tests the pointer against the underlines and the reported card
  // rect every tick. This window ignores mouse events whenever the pointer is
  // over neither, so DOM mouseleave is not reliable here: open and close follow
  // the host's signal only. A short grace covers the poll interval while the
  // pointer travels from the underline into the card.
  useEffect(() => {
    const inside = !!geo.hovered || geo.over_card;
    if (inside) {
      if (closeTimer.current) { window.clearTimeout(closeTimer.current); closeTimer.current = null; }
      if (geo.hovered && geo.hovered !== open) setOpen(geo.hovered);
    } else if (open && !closeTimer.current) {
      closeTimer.current = window.setTimeout(() => { closeTimer.current = null; setOpen(null); }, 400);
    }
  }, [geo.hovered, geo.over_card, open]);

  // Close when the open alert's underline is gone (accepted, dismissed, re-linted away).
  useEffect(() => {
    if (open && geo.rects.length > 0 && !geo.rects.some((r) => r.id === open)) setOpen(null);
  }, [geo.rects, open]);

  // Report the card rect plus a bridge from the underline to the card, so the host
  // keeps this window live (and "over_card" true) while the pointer crosses the gap.
  useEffect(() => {
    if (!open) { ipc.setCardRects([]).catch(() => {}); return; }
    const el = cardRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const rects = [{ x: r.left, y: r.top, w: r.width, h: r.height }];
    const a = geo.rects.find((x) => x.id === open);
    if (a) {
      const top = Math.min(a.y, r.top);
      const bottom = Math.max(a.y + a.h, r.top + r.height);
      rects.push({ x: Math.min(a.x, r.left), y: top, w: Math.max(a.w, 48), h: bottom - top });
    }
    ipc.setCardRects(rects).catch(() => {});
  }, [open, geo.width, geo.height, geo.rects]);

  const alert = open ? alerts[open] : undefined;
  const anchor = open ? geo.rects.find((r) => r.id === open) : undefined;
  let cardStyle: React.CSSProperties = {};
  if (anchor) {
    const left = Math.max(4, Math.min(anchor.x, geo.width - CARD_W - 4));
    const below = anchor.y + anchor.h + 8;
    const top = below + 150 < geo.height ? below : Math.max(4, anchor.y - 160);
    cardStyle = { left, top };
  }

  return (
    <div className="ul-layer">
      {[...geo.rects.filter((r) => r.sentence), ...geo.rects.filter((r) => !r.sentence)].map((r, i) => (
        <div key={r.id + i} className={"ul" + (r.sentence ? " sentence" : "")} style={{ left: r.x, top: r.y + r.h - (r.sentence ? 1 : 2), width: r.w, background: r.color, opacity: open === r.id ? 1 : r.sentence ? 0.55 : 0.9 }} />
      ))}
      {alert && anchor && (
        <div ref={cardRef} className="ul-card" style={cardStyle}>
          <div style={{ display: "flex", alignItems: "center", gap: 8, color: "#bcbcbc", fontSize: 13 }}>
            <i style={{ width: 10, height: 10, borderRadius: 5, background: anchor.color, display: "inline-block" }} />
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
          <div style={{ display: "flex", gap: 12, alignItems: "center" }}>
            {alert.suggestions.length > 0 && (
              <button className="btn btn--primary" style={{ padding: "8px 14px" }} onClick={() => { ipc.acceptAlert(alert.id, 0).catch(() => {}); setOpen(null); }}>Accept</button>
            )}
            {alert.suggestions.length === 0 && (
              <button className="btn btn--outline" style={{ padding: "8px 14px" }} disabled={suggesting === alert.id} onClick={() => { setSuggesting(alert.id); ipc.runFeature("word_options", { id: alert.id }).catch(() => {}).finally(() => setSuggesting(null)); }}>
                {suggesting === alert.id ? "Suggesting…" : "Suggest options"}
              </button>
            )}
            <button className="btn btn--ghost" onClick={() => { ipc.dismissAlert(alert.id).catch(() => {}); setOpen(null); }}>Dismiss</button>
          </div>
          {alert.suggestions.length > 1 && (
            <div style={{ display: "flex", gap: 6, flexWrap: "wrap", marginTop: 10 }}>
              {alert.suggestions.slice(1, 4).map((s, i) => <button key={i} className="btn btn--outline" style={{ padding: "5px 10px" }} onClick={() => { ipc.acceptAlert(alert.id, i + 1).catch(() => {}); setOpen(null); }}>{s || "Remove"}</button>)}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function cap(s: string) { return s.charAt(0).toUpperCase() + s.slice(1); }
