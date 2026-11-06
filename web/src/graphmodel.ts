// Incremental, index-based evidence graph shared by the WebGL scene, the SVG fallback and the
// table fallback. Pages from the server (or a whole static graph) are appended; node ids are
// deduplicated and edges whose ends have not arrived yet wait until they do. Typed arrays keep
// 100k nodes / 300k edges at a few MB.
import type { GNode, Graph, NodeKind } from "./graph";

export const KINDS: NodeKind[] = ["document", "claim", "subject", "decision", "finding"];
export const TONES = ["crit", "warn", "ok", "info", "base"] as const;
export type Tone = (typeof TONES)[number];
const MARKS = [undefined, "changed", "affected", "unaffected"] as const;

/** One server page of `GET /graph/overview` (also the shape a static graph is fed as). */
export interface GraphPage {
  nodes: GNode[];
  edges: [string, string][];
  next?: string | null;
  claims_total?: number | null;
}

/** What one `add` call appended: node range [from, to) and the newly resolved edges (index pairs). */
export interface Batch {
  from: number;
  to: number;
  edges: Uint32Array;
}

function grow<T extends Uint8Array | Uint32Array>(a: T, need: number): T {
  if (need <= a.length) return a;
  const b = new (a.constructor as new (n: number) => T)(Math.max(need, a.length * 2, 1024));
  b.set(a);
  return b;
}

export class GraphModel {
  readonly ids: string[] = [];
  readonly labels: string[] = [];
  kinds = new Uint8Array(1024);
  tones = new Uint8Array(1024);
  marks = new Uint8Array(1024);
  edges = new Uint32Array(2048);
  edgeCount = 0;
  /** Expected total node count (server hint), used to size the layout before everything arrives. */
  expected = 0;
  done = false;
  private index = new Map<string, number>();
  private pending = new Map<string, [string, string][]>();
  private edgeSeen = new Set<number>();
  private listeners = new Set<(b: Batch) => void>();
  private batches: Batch[] = [];

  get size(): number {
    return this.ids.length;
  }

  indexOf(id: string): number | undefined {
    return this.index.get(id);
  }

  kind(i: number): NodeKind {
    return KINDS[this.kinds[i]];
  }

  tone(i: number): Tone {
    return TONES[this.tones[i]];
  }

  mark(i: number): GNode["mark"] {
    return MARKS[this.marks[i]];
  }

  count(kind: NodeKind): number {
    const k = KINDS.indexOf(kind);
    let n = 0;
    for (let i = 0; i < this.size; i++) if (this.kinds[i] === k) n++;
    return n;
  }

  /** Appends a page; returns the batch and notifies subscribers. */
  add(nodes: GNode[], edges: [string, string][]): Batch {
    const from = this.size;
    for (const n of nodes) {
      if (this.index.has(n.id)) continue;
      const i = this.ids.length;
      this.index.set(n.id, i);
      this.ids.push(n.id);
      this.labels.push(n.label);
      this.kinds = grow(this.kinds, i + 1);
      this.tones = grow(this.tones, i + 1);
      this.marks = grow(this.marks, i + 1);
      this.kinds[i] = Math.max(0, KINDS.indexOf(n.kind));
      this.tones[i] = Math.max(0, TONES.indexOf(n.tone));
      this.marks[i] = Math.max(0, MARKS.indexOf(n.mark));
    }
    const start = this.edgeCount;
    const link = (a: string, b: string) => {
      const ia = this.index.get(a);
      const ib = this.index.get(b);
      if (ia === undefined || ib === undefined) {
        // Hold until the missing end arrives (it may be on a later page).
        const miss = ia === undefined ? a : b;
        const list = this.pending.get(miss);
        if (list) list.push([a, b]);
        else this.pending.set(miss, [[a, b]]);
        return;
      }
      const key = ia * 4194304 + ib; // exact in a double for < 4M nodes
      if (ia === ib || this.edgeSeen.has(key)) return;
      this.edgeSeen.add(key);
      this.edges = grow(this.edges, (this.edgeCount + 1) * 2);
      this.edges[this.edgeCount * 2] = ia;
      this.edges[this.edgeCount * 2 + 1] = ib;
      this.edgeCount++;
    };
    for (let i = from; i < this.size; i++) {
      const waiting = this.pending.get(this.ids[i]);
      if (waiting) {
        this.pending.delete(this.ids[i]);
        waiting.forEach(([a, b]) => link(a, b));
      }
    }
    edges.forEach(([a, b]) => link(a, b));
    const batch: Batch = { from, to: this.size, edges: this.edges.slice(start * 2, this.edgeCount * 2) };
    this.batches.push(batch);
    this.listeners.forEach((f) => f(batch));
    return batch;
  }

  /** Subscribes to future batches after replaying the ones already added. */
  subscribe(f: (b: Batch) => void): () => void {
    this.batches.forEach(f);
    this.listeners.add(f);
    return () => this.listeners.delete(f);
  }

  /** Neighbour indices of node i (scan; called on selection only). */
  neighbours(i: number, max = 24): number[] {
    const out: number[] = [];
    for (let e = 0; e < this.edgeCount && out.length < max; e++) {
      if (this.edges[e * 2] === i) out.push(this.edges[e * 2 + 1]);
      else if (this.edges[e * 2 + 1] === i) out.push(this.edges[e * 2]);
    }
    return out;
  }

  degree(i: number): number {
    let d = 0;
    for (let e = 0; e < this.edgeCount * 2; e++) if (this.edges[e] === i) d++;
    return d;
  }

  /** Plain graph view (SVG fallback for small graphs). */
  toGraph(): Graph {
    const nodes: GNode[] = this.ids.map((id, i) => ({ id, kind: this.kind(i), label: this.labels[i], tone: this.tone(i), mark: this.mark(i) }));
    const edges: [string, string][] = [];
    for (let e = 0; e < this.edgeCount; e++) edges.push([this.ids[this.edges[e * 2]], this.ids[this.edges[e * 2 + 1]]]);
    return { nodes, edges };
  }

  static of(g: Graph): GraphModel {
    const m = new GraphModel();
    m.expected = g.nodes.length;
    m.add(g.nodes, g.edges);
    m.done = true;
    return m;
  }
}

export type PageFetcher = (after: string | null, signal: AbortSignal) => Promise<GraphPage>;

/**
 * Streams pages into a model until the server reports no `next`. `onProgress` runs after each
 * page; the returned promise settles when loading finishes, fails or is aborted.
 */
export async function streamPages(m: GraphModel, fetchPage: PageFetcher, signal: AbortSignal, onProgress: () => void): Promise<void> {
  let after: string | null = null;
  for (;;) {
    const page: GraphPage = await fetchPage(after, signal);
    if (signal.aborted) return;
    if (after === null) {
      const extra = page.nodes.filter((n) => n.kind !== "claim" && n.kind !== "subject").length;
      // Claims plus roughly one record per few claims plus the documents/decisions/findings page.
      m.expected = Math.max(page.nodes.length, extra + Math.round((page.claims_total ?? 0) * 1.15));
    }
    m.add(page.nodes, page.edges);
    if (!page.next) {
      m.done = true;
      onProgress();
      return;
    }
    onProgress();
    after = page.next;
  }
}
