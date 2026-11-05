import { useEffect, useRef, useState } from "react";
import { webglUsable } from "./graph";

/** Original KIZUNA mark: two interlocking strokes forming a knot (絆, "bond"). */
export function KnotMark({ size = 28, title }: { size?: number; title?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 64 64" className="knot" role={title ? "img" : undefined} aria-hidden={title ? undefined : true} aria-label={title}>
      <rect x="6" y="18" width="34" height="28" rx="14" className="knot-a" />
      <rect x="24" y="18" width="34" height="28" rx="14" className="knot-b" />
      {/* over-stroke: strand A passes over strand B at the upper crossing */}
      <path d="M33 18.6 A14 14 0 0 1 40 30" className="knot-gap" />
      <path d="M33 18.6 A14 14 0 0 1 40 30" className="knot-a" />
    </svg>
  );
}

/** Login hero: lazy three.js faceted knot with mouse parallax; static mark when motion/WebGL unavailable. */
export function KnotHero() {
  const ref = useRef<HTMLCanvasElement>(null);
  const [gl] = useState(webglUsable);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!gl || !ref.current) return;
    let dispose: (() => void) | undefined;
    let dead = false;
    import("./scene3d")
      .then((m) => {
        if (!dead && ref.current) dispose = m.mountKnot(ref.current);
      })
      .catch(() => setFailed(true));
    return () => {
      dead = true;
      dispose?.();
    };
  }, [gl]);
  return (
    <div className="hero-scene" aria-hidden="true">
      {gl && !failed ? <canvas ref={ref} /> : <KnotMark size={320} />}
    </div>
  );
}
