// Layout Web Worker: keeps layout, clustering and chunking off the main thread. Receives each
// model batch (kinds/tones of the new nodes + new edge pairs) and answers with transferable arrays.
import { Layout, type LayoutOutput } from "./graphlayout";

export interface LayoutRequest {
  seq: number;
  from: number;
  to: number;
  kinds: Uint8Array; // nodes [from, to)
  tones: Uint8Array;
  edges: Uint32Array;
  expected: number;
}
export type LayoutReply = LayoutOutput & { seq: number; ms: number };

const layout = new Layout();
let kinds = new Uint8Array(0);
let tones = new Uint8Array(0);

self.onmessage = (e: MessageEvent<LayoutRequest>) => {
  const t0 = performance.now();
  const r = e.data;
  if (r.to > kinds.length) {
    const k = new Uint8Array(Math.max(r.to, kinds.length * 2));
    const t = new Uint8Array(k.length);
    k.set(kinds);
    t.set(tones);
    kinds = k;
    tones = t;
  }
  kinds.set(r.kinds, r.from);
  tones.set(r.tones, r.from);
  const out = layout.add(kinds, tones, r.from, r.to, r.edges, r.expected);
  const reply: LayoutReply = { ...out, seq: r.seq, ms: performance.now() - t0 };
  (self as unknown as Worker).postMessage(reply, [
    out.pos.buffer,
    out.scale.buffer,
    out.chunk.buffer,
    out.cPos.buffer,
    out.cScale.buffer,
    out.cTone.buffer,
    out.cRep.buffer,
    out.farEdges.buffer,
    out.bounds.buffer,
  ]);
};
