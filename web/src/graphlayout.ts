// Deterministic, incremental layered layout for the evidence graph, plus level-of-detail clusters,
// spatial chunks (frustum culling) and a uniform-grid spatial index (picking). Pure TypeScript with
// no three.js, so it runs in the layout Web Worker and in unit tests.
//
// Layers run left to right along x: documents → claims → records (parts/suppliers) → decisions and
// findings. Within a layer, nodes sit on a sunflower (Vogel) spiral in the y–z plane, so positions
// depend only on arrival order and a seed: nodes already placed never move when later pages arrive,
// except decisions/findings, which follow the barycentre of the evidence they are linked to.
// Claims gather around their record's spiral slot, which is what makes clustering by record
// (subject/part) both a visual grouping and the far-zoom level of detail.

/** Kind indices as in graphmodel.ts KINDS: document, claim, subject, decision, finding. */
const DOC = 0, CLAIM = 1, SUBJ = 2, DEC = 3, FND = 4;
const LAYER = [0, 1, 2, 3, 3];
const SCALE = [0.8, 0.8, 1.1, 1.4, 1.4];
const GOLDEN = Math.PI * (3 - Math.sqrt(5));
/** Tone priority for a cluster (worst member wins): index into TONES crit, warn, ok, info, base. */
const TONE_RANK = [0, 1, 2, 3, 4];

/** 32-bit FNV-1a mixed with the seed; deterministic per node. */
function hash(i: number, seed: number): number {
  let h = (2166136261 ^ seed) >>> 0;
  h = Math.imul(h ^ (i & 0xff), 16777619);
  h = Math.imul(h ^ ((i >>> 8) & 0xff), 16777619);
  h = Math.imul(h ^ ((i >>> 16) & 0xff), 16777619);
  h = Math.imul(h ^ (i >>> 24), 16777619);
  return (h >>> 0) / 4294967296;
}

function growF(a: Float32Array, need: number): Float32Array {
  if (need <= a.length) return a;
  const b = new Float32Array(Math.max(need, a.length * 2, 1024));
  b.set(a);
  return b;
}
function growI(a: Int32Array, need: number, fill = 0): Int32Array {
  if (need <= a.length) return a;
  const b = new Int32Array(Math.max(need, a.length * 2, 1024)).fill(fill);
  b.set(a);
  return b;
}

export interface LayoutOutput {
  n: number;
  /** xyz per node. */
  pos: Float32Array;
  /** Uniform scale per node. */
  scale: Float32Array;
  /** Spatial chunk per node (frustum-culled together). */
  chunk: Uint16Array;
  chunks: number;
  /** Cluster (one per record with claims): centre xyz, scale, tone index, representative node. */
  cPos: Float32Array;
  cScale: Float32Array;
  cTone: Uint8Array;
  cRep: Uint32Array;
  /** Edges for the far level of detail (index pairs; ends ≥ n refer to cluster n − cluster index). */
  farEdges: Uint32Array;
  /** Axis-aligned bounds [minx, miny, minz, maxx, maxy, maxz]. */
  bounds: Float32Array;
}

export class Layout {
  private n = 0;
  private pos = new Float32Array(3072);
  private kinds = new Uint8Array(0);
  private tones = new Uint8Array(0);
  /** claim → subject node index (−1 none). */
  private clusterOf = new Int32Array(1024).fill(-1);
  /** subject node → spiral slot; claims per subject. */
  private slotOf = new Int32Array(1024).fill(-1);
  private members = new Int32Array(1024);
  private slots = [0, 0, 0, 0];
  /** decision/finding barycentre accumulators (y, z, count) and ordinal. */
  private acc = new Float32Array(3072);
  private ord = new Int32Array(1024);
  private gap = 6;
  private docSpacing = 1.6;
  private sized = false;
  private farKeys = new Set<number>();
  private far: number[] = [];

  constructor(private seed = 0x6b697a75) {}

  private spiral(slot: number, spacing: number, out: Float32Array, o: number) {
    const r = spacing * Math.sqrt(slot + 0.5);
    const a = slot * GOLDEN;
    out[o + 1] = r * Math.cos(a);
    out[o + 2] = r * Math.sin(a);
  }

  private place(i: number) {
    const k = this.kinds[i];
    const o = i * 3;
    const p = this.pos;
    p[o] = (LAYER[k] - 1.5) * this.gap + (hash(i, this.seed) - 0.5) * 0.6;
    if (k === SUBJ) {
      this.slotOf[i] = this.slots[2]++;
      this.spiral(this.slotOf[i], 1.25, p, o);
    } else if (k === CLAIM && this.clusterOf[i] >= 0) {
      const s = this.clusterOf[i];
      const m = this.members[s]++;
      // Local sunflower around the record's slot: same y–z as the record one layer to the right.
      const r = 0.34 * Math.sqrt(m + 1);
      const a = m * GOLDEN + hash(s, this.seed) * Math.PI * 2;
      p[o + 1] = p[s * 3 + 1] + r * Math.cos(a);
      p[o + 2] = p[s * 3 + 2] + r * Math.sin(a);
    } else if (k === CLAIM) {
      this.spiral(this.slots[1]++, 1.25, p, o);
    } else if (k === DOC) {
      this.spiral(this.slots[0]++, this.docSpacing, p, o);
    } else {
      this.ord[i] = this.slots[3]++;
      this.spiral(this.ord[i], 1.6, p, o);
    }
  }

  /**
   * Adds nodes [from, to) (kinds/tones are the model's full arrays) and newly resolved edges.
   * `expected` sizes the layer gap once, on the first call, so later pages keep their positions.
   */
  add(kinds: Uint8Array, tones: Uint8Array, from: number, to: number, edges: Uint32Array, expected: number): LayoutOutput {
    this.kinds = kinds;
    this.tones = tones;
    this.n = to;
    this.pos = growF(this.pos, to * 3);
    this.acc = growF(this.acc, to * 3);
    this.clusterOf = growI(this.clusterOf, to, -1);
    this.slotOf = growI(this.slotOf, to, -1);
    this.members = growI(this.members, to);
    this.ord = growI(this.ord, to);
    if (!this.sized) {
      this.sized = true;
      const total = Math.max(expected, to);
      this.gap = Math.max(6, 0.5 * Math.sqrt(total));
      let docs = 0;
      for (let i = from; i < to; i++) if (kinds[i] === DOC) docs++;
      const radius = 1.25 * Math.sqrt(total / 9);
      this.docSpacing = Math.max(1.6, radius / Math.sqrt(Math.max(1, docs)));
    }
    // Cluster membership first, so new claims are placed beside their record.
    for (let e = 0; e < edges.length; e += 2) {
      const a = edges[e], b = edges[e + 1];
      if (kinds[a] === CLAIM && kinds[b] === SUBJ && this.clusterOf[a] < 0 && a >= from) this.clusterOf[a] = b;
      else if (kinds[b] === CLAIM && kinds[a] === SUBJ && this.clusterOf[b] < 0 && b >= from) this.clusterOf[b] = a;
    }
    // Records before claims (claims need their record's slot), then everything else.
    for (let i = from; i < to; i++) if (kinds[i] === SUBJ) this.place(i);
    for (let i = from; i < to; i++) if (kinds[i] !== SUBJ) this.place(i);
    // Decisions and findings follow the barycentre of their linked evidence.
    const touched = new Set<number>();
    const p = this.pos;
    for (let e = 0; e < edges.length; e += 2) {
      for (const [x, y] of [[edges[e], edges[e + 1]], [edges[e + 1], edges[e]]]) {
        if ((kinds[x] === DEC || kinds[x] === FND) && kinds[y] !== DEC && kinds[y] !== FND) {
          this.acc[x * 3] += p[y * 3 + 1];
          this.acc[x * 3 + 1] += p[y * 3 + 2];
          this.acc[x * 3 + 2] += 1;
          touched.add(x);
        }
      }
    }
    touched.forEach((x) => {
      const c = this.acc[x * 3 + 2];
      const j = this.ord[x] % 13;
      const r = 0.55 * Math.sqrt(j);
      p[x * 3 + 1] = this.acc[x * 3] / c + r * Math.cos(j * GOLDEN);
      p[x * 3 + 2] = this.acc[x * 3 + 1] / c + r * Math.sin(j * GOLDEN);
    });
    this.addFarEdges(edges);
    return this.output();
  }

  /** Far-LOD edges: claim ends collapse into their record's cluster; duplicates dropped. */
  private addFarEdges(edges: Uint32Array) {
    const K = 4194304;
    const end = (i: number) => (this.kinds[i] === CLAIM ? (this.clusterOf[i] >= 0 ? -1 - this.clusterOf[i] : null) : i);
    for (let e = 0; e < edges.length; e += 2) {
      const a = end(edges[e]), b = end(edges[e + 1]);
      if (a === null || b === null || a === b) continue;
      // Store cluster ends as negative (−1 − record index); resolved to cluster ids in output().
      const key = (a + K) * 2 * K + (b + K);
      if (this.farKeys.has(key)) continue;
      this.farKeys.add(key);
      this.far.push(a, b);
    }
  }

  private output(): LayoutOutput {
    const n = this.n;
    const pos = this.pos.slice(0, n * 3);
    const scale = new Float32Array(n);
    const bounds = new Float32Array([Infinity, Infinity, Infinity, -Infinity, -Infinity, -Infinity]);
    for (let i = 0; i < n; i++) {
      scale[i] = SCALE[this.kinds[i]] ?? 0.8;
      for (let d = 0; d < 3; d++) {
        bounds[d] = Math.min(bounds[d], pos[i * 3 + d]);
        bounds[d + 3] = Math.max(bounds[d + 3], pos[i * 3 + d]);
      }
    }
    if (!n) bounds.fill(0);
    // Chunks: a y–z grid per layer (roughly 16×16 cells for a full layer) for frustum culling.
    const cell = Math.max(8, this.gap * 0.12);
    const chunkIds = new Map<number, number>();
    const chunk = new Uint16Array(n);
    for (let i = 0; i < n; i++) {
      const cy = Math.floor(pos[i * 3 + 1] / cell) + 512;
      const cz = Math.floor(pos[i * 3 + 2] / cell) + 512;
      const key = (LAYER[this.kinds[i]] * 1024 + cy) * 1024 + cz;
      let c = chunkIds.get(key);
      if (c === undefined) chunkIds.set(key, (c = chunkIds.size));
      chunk[i] = Math.min(c, 65535);
    }
    // Clusters: one per record that has claims, at the centre of its claims.
    const clusterIdx = new Map<number, number>();
    const rep: number[] = [];
    for (let i = 0; i < n; i++) if (this.kinds[i] === SUBJ && this.members[i] > 0) (clusterIdx.set(i, rep.length), rep.push(i));
    const m = rep.length;
    const cPos = new Float32Array(m * 3);
    const cScale = new Float32Array(m);
    const cTone = new Uint8Array(m).fill(4);
    const cRep = new Uint32Array(rep);
    rep.forEach((s, c) => {
      cPos[c * 3] = (LAYER[CLAIM] - 1.5) * this.gap;
      cPos[c * 3 + 1] = pos[s * 3 + 1];
      cPos[c * 3 + 2] = pos[s * 3 + 2];
      cScale[c] = Math.min(4, 0.8 * Math.sqrt(this.members[s]) * 0.6 + 0.6);
    });
    for (let i = 0; i < n; i++) {
      const s = this.clusterOf[i];
      if (this.kinds[i] !== CLAIM || s < 0) continue;
      const c = clusterIdx.get(s);
      if (c !== undefined && TONE_RANK[this.tones[i]] < TONE_RANK[cTone[c]]) cTone[c] = this.tones[i];
    }
    const far: number[] = [];
    for (let e = 0; e < this.far.length; e += 2) {
      const r = (x: number) => {
        if (x >= 0) return x;
        const c = clusterIdx.get(-1 - x);
        return c === undefined ? -1 : n + c;
      };
      const a = r(this.far[e]), b = r(this.far[e + 1]);
      if (a >= 0 && b >= 0 && a !== b) far.push(a, b);
    }
    return { n, pos, scale, chunk, chunks: chunkIds.size, cPos, cScale, cTone, cRep, farEdges: new Uint32Array(far), bounds };
  }
}

/**
 * Uniform-grid spatial index over node centres. `pick` walks the cells a ray crosses (3D DDA) and
 * returns the nearest node whose bounding sphere the ray hits — O(cells crossed), not O(n).
 */
export class SpatialGrid {
  private cells = new Map<number, number[]>();
  private min: [number, number, number];
  private dims: [number, number, number];

  constructor(
    private pos: Float32Array,
    private radius: Float32Array,
    count: number,
    private cell = 2,
  ) {
    const lo: [number, number, number] = [Infinity, Infinity, Infinity];
    const hi: [number, number, number] = [-Infinity, -Infinity, -Infinity];
    for (let i = 0; i < count; i++)
      for (let d = 0; d < 3; d++) {
        lo[d] = Math.min(lo[d], pos[i * 3 + d]);
        hi[d] = Math.max(hi[d], pos[i * 3 + d]);
      }
    if (!count) lo.fill(0), hi.fill(0);
    let maxR = 0;
    for (let i = 0; i < count; i++) maxR = Math.max(maxR, radius[i]);
    this.cell = Math.max(cell, maxR * 2);
    this.min = [lo[0] - this.cell, lo[1] - this.cell, lo[2] - this.cell];
    this.dims = [0, 1, 2].map((d) => Math.max(1, Math.ceil((hi[d] - this.min[d]) / this.cell) + 2)) as [number, number, number];
    for (let i = 0; i < count; i++) {
      const k = this.key(this.coord(pos, i * 3));
      const list = this.cells.get(k);
      if (list) list.push(i);
      else this.cells.set(k, [i]);
    }
  }

  private coord(p: ArrayLike<number>, o: number): [number, number, number] {
    return [0, 1, 2].map((d) => Math.floor((p[o + d] - this.min[d]) / this.cell)) as [number, number, number];
  }

  private key([x, y, z]: [number, number, number]): number {
    return (x * this.dims[1] + y) * this.dims[2] + z;
  }

  /** Nearest node hit by the ray origin + t·dir (dir normalised), or −1. */
  pick(origin: [number, number, number], dir: [number, number, number], maxSteps = 6000): number {
    // Clip the ray to the grid box, then step cell by cell.
    let t0 = 0, t1 = Infinity;
    for (let d = 0; d < 3; d++) {
      const lo = this.min[d], hi = this.min[d] + this.dims[d] * this.cell;
      if (Math.abs(dir[d]) < 1e-12) {
        if (origin[d] < lo || origin[d] > hi) return -1;
        continue;
      }
      let a = (lo - origin[d]) / dir[d], b = (hi - origin[d]) / dir[d];
      if (a > b) [a, b] = [b, a];
      t0 = Math.max(t0, a);
      t1 = Math.min(t1, b);
    }
    if (t0 > t1) return -1;
    const start = [0, 1, 2].map((d) => origin[d] + dir[d] * (t0 + 1e-6));
    const c = this.coord(start, 0).map((v, d) => Math.min(this.dims[d] - 1, Math.max(0, v))) as [number, number, number];
    const step = dir.map((v) => (v > 0 ? 1 : v < 0 ? -1 : 0));
    const tMax = [0, 1, 2].map((d) => {
      if (!step[d]) return Infinity;
      const edge = this.min[d] + (c[d] + (step[d] > 0 ? 1 : 0)) * this.cell;
      return (edge - origin[d]) / dir[d];
    });
    const tDelta = dir.map((v) => (v ? this.cell / Math.abs(v) : Infinity));
    let best = -1, bestT = Infinity;
    // Spheres can overlap the neighbouring cell, so test the 3×3×3 block around each visited cell.
    const seen = new Set<number>();
    for (let s = 0; s < maxSteps; s++) {
      for (let dx = -1; dx <= 1; dx++)
        for (let dy = -1; dy <= 1; dy++)
          for (let dz = -1; dz <= 1; dz++) {
            const q: [number, number, number] = [c[0] + dx, c[1] + dy, c[2] + dz];
            if (q.some((v, d) => v < 0 || v >= this.dims[d])) continue;
            const k = this.key(q);
            if (seen.has(k)) continue;
            seen.add(k);
            for (const i of this.cells.get(k) ?? []) {
              const ox = this.pos[i * 3] - origin[0], oy = this.pos[i * 3 + 1] - origin[1], oz = this.pos[i * 3 + 2] - origin[2];
              const t = ox * dir[0] + oy * dir[1] + oz * dir[2];
              if (t < 0) continue;
              const d2 = ox * ox + oy * oy + oz * oz - t * t;
              const r = this.radius[i];
              if (d2 <= r * r && t < bestT) (bestT = t), (best = i);
            }
          }
      // Anything found closer than the next cell boundary cannot be beaten further along.
      const next = Math.min(tMax[0], tMax[1], tMax[2]);
      if (best >= 0 && bestT < next - this.cell) break;
      if (next > t1) break;
      const d = tMax[0] === next ? 0 : tMax[1] === next ? 1 : 2;
      c[d] += step[d];
      if (c[d] < 0 || c[d] >= this.dims[d]) break;
      tMax[d] += tDelta[d];
    }
    return best;
  }
}
