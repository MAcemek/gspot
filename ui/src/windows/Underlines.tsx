import { useEffect, useState } from "react";
import { on } from "../lib/ipc";
import type { Geometry } from "../lib/types";

/** Draws the underlines only; the hover card lives in its own window (Card.tsx). */
export function Underlines() {
  const [geo, setGeo] = useState<Geometry>({ origin_x: 0, origin_y: 0, width: 0, height: 0, rects: [], hovered: null, over_card: false });

  useEffect(() => {
    const u = on<Geometry>("host://geometry", setGeo);
    return () => { u.then((f) => f()); };
  }, []);

  return (
    <div className="ul-layer">
      {[...geo.rects.filter((r) => r.sentence), ...geo.rects.filter((r) => !r.sentence)].map((r, i) => (
        <div key={r.id + i} className={"ul" + (r.sentence ? " sentence" : "")} style={{ left: r.x, top: r.y + r.h - (r.sentence ? 1 : 2), width: r.w, background: r.color, opacity: geo.hovered === r.id ? 1 : r.sentence ? 0.55 : 0.9 }} />
      ))}
    </div>
  );
}
