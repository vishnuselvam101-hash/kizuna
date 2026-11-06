// Lazy-loaded WebGL scenes (three.js). Imported only via dynamic import (graph.tsx, brand.tsx), so the
// app shell bundle never contains three.js. Rendering pauses when the tab is hidden or the
// canvas is off-screen; transform-only motion, no per-frame allocation.
import * as THREE from "three";
import type { GraphModel } from "./graphmodel";
import { SpatialGrid } from "./graphlayout";
import type { LayoutReply, LayoutRequest } from "./graph.worker";

export type Dispose = () => void;

function token(name: string, fallback: string): THREE.Color {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return new THREE.Color(v || fallback);
}

function setup(canvas: HTMLCanvasElement, antialias = true) {
  const renderer = new THREE.WebGLRenderer({ canvas, antialias, alpha: true, powerPreference: "low-power" });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 1.5));
  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(40, 1, 0.1, 200);
  const resize = () => {
    const w = canvas.clientWidth || 1;
    const h = canvas.clientHeight || 1;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  };
  resize();
  const ro = new ResizeObserver(() => {
    resize();
    kick();
  });
  ro.observe(canvas);
  scene.add(new THREE.AmbientLight(0xffffff, 0.35));
  const key = new THREE.DirectionalLight(0xffffff, 1.5);
  key.position.set(4, 6, 8);
  const rim = new THREE.DirectionalLight(token("--info", "#3fd0c9"), 0.5);
  rim.position.set(-6, -2, -4);
  scene.add(key, rim);

  let visible = true;
  let raf = 0;
  // frame() returns true to keep animating; otherwise rendering waits for kick() (on demand).
  let frame: (t: number) => boolean = () => true;
  const loop = (t: number) => {
    raf = 0;
    const more = frame(t);
    renderer.render(scene, camera);
    if (more && visible && !document.hidden) raf = requestAnimationFrame(loop);
  };
  const kick = () => {
    if (!raf && visible && !document.hidden) raf = requestAnimationFrame(loop);
  };
  const io = new IntersectionObserver(([e]) => {
    visible = e.isIntersecting;
    kick();
  });
  io.observe(canvas);
  document.addEventListener("visibilitychange", kick);

  return {
    renderer,
    scene,
    camera,
    start(f: (t: number) => boolean) {
      frame = f;
      kick();
    },
    kick,
    dispose() {
      cancelAnimationFrame(raf);
      io.disconnect();
      ro.disconnect();
      document.removeEventListener("visibilitychange", kick);
      scene.traverse((o) => {
        const m = o as THREE.Mesh;
        m.geometry?.dispose();
        const mat = m.material as THREE.Material | THREE.Material[] | undefined;
        (Array.isArray(mat) ? mat : mat ? [mat] : []).forEach((x) => x.dispose());
      });
      renderer.dispose();
    },
  };
}

/** Faceted knot mark: two interlocking torus-knot strokes (絆, "bond"). */
export function mountKnot(canvas: HTMLCanvasElement): Dispose {
  const s = setup(canvas);
  s.camera.position.set(0, 0.6, 13);
  const wide = canvas.clientWidth > canvas.clientHeight;
  const group = new THREE.Group();
  const mk = (color: THREE.Color, rot: number) => {
    const geo = new THREE.TorusKnotGeometry(1.6, 0.34, 96, 6, 2, 3);
    const mesh = new THREE.Mesh(geo, new THREE.MeshStandardMaterial({ color, flatShading: true, metalness: 0.4, roughness: 0.35 }));
    mesh.rotation.z = rot;
    const edges = new THREE.LineSegments(new THREE.EdgesGeometry(geo, 25), new THREE.LineBasicMaterial({ color: 0xffffff, transparent: true, opacity: 0.12 }));
    mesh.add(edges);
    return mesh;
  };
  group.add(mk(token("--surface-2", "#1a2340"), 0), mk(token("--gold", "#d8b26a"), Math.PI / 2));
  group.children[1].scale.setScalar(0.62);
  group.position.set(wide ? 0.5 : 0.4, wide ? 1.2 : 2.2, 0);
  group.scale.setScalar(wide ? 1 : 0.75);
  s.scene.add(group);
  const grid = new THREE.GridHelper(40, 40, token("--info", "#3fd0c9"), 0x223050);
  (grid.material as THREE.Material).transparent = true;
  (grid.material as THREE.Material).opacity = 0.25;
  grid.position.y = -3;
  s.scene.add(grid);
  const mouse = { x: 0, y: 0 };
  const onMove = (e: PointerEvent) => {
    mouse.x = e.clientX / window.innerWidth - 0.5;
    mouse.y = e.clientY / window.innerHeight - 0.5;
  };
  window.addEventListener("pointermove", onMove, { passive: true });
  s.start((t) => {
    group.rotation.y = t * 0.00018;
    group.rotation.x = 0.25 + mouse.y * 0.3;
    s.camera.position.x += (mouse.x * 2 - s.camera.position.x) * 0.04;
    s.camera.lookAt(0, 0, 0);
    return true;
  });
  return () => {
    window.removeEventListener("pointermove", onMove);
    s.dispose();
  };
}

/** Above this many nodes the far view collapses claims into one marker per record. */
const LOD_MIN = 4000;
/** Small graphs keep the gentle idle drift (when motion is allowed); large ones render on demand. */
const DRIFT_MAX = 3000;
/** Most links drawn in the far view (an even sample of the record-level links). */
const FAR_EDGES = 20000;

export interface GraphStats {
  nodes: number;
  edges: number;
  lod: "near" | "far";
  drawCalls: number;
  layoutMs: number;
  rebuildMs: number;
  /** performance.now() when the first nodes were drawn / when the last page was drawn. */
  firstDrawAt: number;
  lastDrawAt: number;
  frames: number;
}

export interface GraphView {
  dispose(): void;
  /** Marks node i (or nothing) as selected; `focus` also moves the camera to it. */
  select(i: number | null, focus?: boolean): void;
  orbit(dAz: number, dEl: number): void;
  zoom(factor: number): void;
  reset(): void;
  stats: GraphStats;
}

export interface GraphHandlers {
  onSelect(i: number): void;
  onOpen(i: number): void;
  onHover(i: number | null, x: number, y: number): void;
}

/**
 * Scalable evidence graph: InstancedMesh nodes and merged LineSegments edges in spatial chunks
 * (frustum-culled), layout in a Web Worker, far-zoom clusters per record, grid-indexed picking,
 * on-demand rendering that stops when hidden. Streams batches from the model as pages arrive.
 */
export function mountGraph(canvas: HTMLCanvasElement, model: GraphModel, h: GraphHandlers, highlightIds: ReadonlySet<string>, motion: boolean): GraphView {
  // Large graphs: no MSAA and Lambert shading — fill rate dominates (especially in software GL).
  const big = Math.max(model.expected, model.size) > LOD_MIN;
  const s = setup(canvas, !big);
  const root = new THREE.Group();
  s.scene.add(root);
  const tones = [token("--crit", "#ff4d2e"), token("--warn", "#e7a33e"), token("--gold", "#d8b26a"), token("--info", "#3fd0c9"), token("--fg-muted", "#8a95ad")];
  const bg = token("--bg", "#070a14");
  const geo = new THREE.OctahedronGeometry(0.42, 0);
  const nodeMat = big
    ? new THREE.MeshLambertMaterial({ color: 0xffffff, flatShading: true })
    : new THREE.MeshStandardMaterial({ color: 0xffffff, flatShading: true, roughness: 0.4, metalness: 0.3 });
  // Distance fade: when zoomed in, the far plane closes in so chunks behind the view are culled.
  s.scene.fog = new THREE.Fog(bg, 1, 2);
  const hl = highlightIds.size > 0;
  // Dense graphs: links fade with density so they read as structure, not a solid wash.
  const edgeAlpha = () => Math.max(0.025, Math.min(hl ? 0.1 : 0.22, 0.22 * Math.sqrt(3000 / Math.max(1, model.edgeCount))));
  const edgeMat = new THREE.LineBasicMaterial({ color: tones[3], transparent: true, opacity: edgeAlpha(), depthWrite: false });
  const hotMat = new THREE.LineBasicMaterial({ color: tones[0], transparent: true, opacity: 0.9 });
  const selMat = new THREE.LineBasicMaterial({ color: tones[2], transparent: true, opacity: 0.95 });
  const marker = new THREE.LineSegments(new THREE.EdgesGeometry(geo), selMat);
  marker.visible = false;
  s.scene.add(marker);
  let selLines: THREE.LineSegments | null = null;

  const stats: GraphStats = { nodes: 0, edges: 0, lod: "near", drawCalls: 0, layoutMs: 0, rebuildMs: 0, firstDrawAt: 0, lastDrawAt: 0, frames: 0 };
  // Camera: orbit about a target (spherical coordinates); auto-framed until the user moves it.
  const cam = { az: 0.62, el: 0.28, dist: 20, target: new THREE.Vector3() };
  let interacted = false;
  let sceneRadius = 10;
  let data: LayoutReply | null = null;
  let pending: LayoutReply | null = null;
  let lastRebuild = 0;
  let lastCost = 0;
  let nearGrid: SpatialGrid | null = null;
  let farGrid: { grid: SpatialGrid; map: Int32Array } | null = null;
  const near: THREE.Object3D[] = [];
  const far: THREE.Object3D[] = [];
  let selected: number | null = null;

  const applyCamera = () => {
    const c = s.camera;
    const r = cam.dist;
    c.position.set(cam.target.x + r * Math.cos(cam.el) * Math.sin(cam.az), cam.target.y + r * Math.sin(cam.el), cam.target.z + r * Math.cos(cam.el) * Math.cos(cam.az));
    c.near = Math.max(0.05, r * 0.01);
    c.far = r + Math.min(sceneRadius * 2 + 10, Math.max(r * 2, 50));
    const fog = s.scene.fog as THREE.Fog;
    fog.near = c.far * 0.6;
    fog.far = c.far;
    c.updateProjectionMatrix();
    c.lookAt(cam.target);
  };
  const fit = () => {
    if (!data) return;
    const b = data.bounds;
    cam.target.set((b[0] + b[3]) / 2, (b[1] + b[4]) / 2, (b[2] + b[5]) / 2);
    sceneRadius = Math.max(4, Math.hypot(b[3] - b[0], b[4] - b[1], b[5] - b[2]) / 2);
    // Fit the box as seen from the current orbit angle: projected half-width/height, plus half
    // the depth toward the camera (perspective makes the near side larger).
    const ex = (b[3] - b[0]) / 2 + 0.6, ey = (b[4] - b[1]) / 2 + 0.6, ez = (b[5] - b[2]) / 2 + 0.6;
    const sa = Math.abs(Math.sin(cam.az)), ca = Math.abs(Math.cos(cam.az));
    const depth = ex * sa + ez * ca;
    const halfW = ex * ca + ez * sa;
    const halfH = ey * Math.cos(cam.el) + depth * Math.sin(cam.el);
    const tanHalf = Math.tan(THREE.MathUtils.degToRad(s.camera.fov / 2));
    cam.dist = Math.max(halfH / tanHalf, halfW / (tanHalf * s.camera.aspect)) * 0.95 + depth * 0.15;
  };

  const worker = new Worker(new URL("./graph.worker.ts", import.meta.url), { type: "module" });
  let seq = 0;
  worker.onmessage = (e: MessageEvent<LayoutReply>) => {
    pending = e.data;
    stats.layoutMs += e.data.ms;
    s.kick();
  };
  const unsubscribe = model.subscribe((b) => {
    const req: LayoutRequest = { seq: ++seq, from: b.from, to: b.to, kinds: model.kinds.slice(b.from, b.to), tones: model.tones.slice(b.from, b.to), edges: b.edges.slice(), expected: model.expected };
    worker.postMessage(req, [req.kinds.buffer, req.tones.buffer, req.edges.buffer]);
  });

  const clear = (list: THREE.Object3D[]) => {
    list.forEach((o) => {
      root.remove(o);
      (o as THREE.Mesh).geometry !== geo && (o as THREE.Mesh).geometry?.dispose();
      (o as THREE.InstancedMesh).isInstancedMesh && (o as THREE.InstancedMesh).dispose();
    });
    list.length = 0;
  };
  const tmp = new THREE.Color();
  const colourOf = (tone: number, hit: boolean) => {
    tmp.copy(tones[tone] ?? tones[4]);
    if (hl) hit ? tmp.multiplyScalar(1.35) : tmp.lerp(bg, 0.72);
    return tmp;
  };
  const instanced = (pos: Float32Array, scale: Float32Array, tone: ArrayLike<number>, members: ArrayLike<number>, hits: Uint8Array | null) => {
    const m = new THREE.InstancedMesh(geo, nodeMat, members.length);
    const arr = m.instanceMatrix.array as Float32Array;
    const col = new Float32Array(members.length * 3);
    for (let k = 0; k < members.length; k++) {
      const i = members[k];
      const hit = !!hits?.[i];
      const sc = scale[i] * (hit ? 1.3 : 1);
      const o = k * 16; // identity rotation: only scale and translation are written
      arr[o] = arr[o + 5] = arr[o + 10] = sc;
      arr[o + 12] = pos[i * 3];
      arr[o + 13] = pos[i * 3 + 1];
      arr[o + 14] = pos[i * 3 + 2];
      colourOf(tone[i], hit).toArray(col, k * 3);
    }
    m.instanceColor = new THREE.InstancedBufferAttribute(col, 3);
    m.computeBoundingSphere();
    return m;
  };
  const lines = (pts: Float32Array, mat: THREE.LineBasicMaterial) => {
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.BufferAttribute(pts, 3));
    g.computeBoundingSphere();
    return new THREE.LineSegments(g, mat);
  };

  const rebuild = (r: LayoutReply) => {
    const t0 = performance.now();
    data = r;
    edgeMat.opacity = edgeAlpha();
    clear(near);
    clear(far);
    const n = r.n;
    const hits = hl ? new Uint8Array(n) : null;
    if (hits) highlightIds.forEach((id) => {
      const i = model.indexOf(id);
      if (i !== undefined && i < n) hits[i] = 1;
    });
    for (let i = 0; hits && i < n; i++) {
      const mk = model.mark(i);
      if (mk === "changed" || mk === "affected") hits[i] = 1;
    }
    // Nodes: one InstancedMesh per spatial chunk.
    const lists: number[][] = Array.from({ length: r.chunks }, () => []);
    for (let i = 0; i < n; i++) lists[r.chunk[i]].push(i);
    lists.forEach((members) => {
      if (!members.length) return;
      const mesh = instanced(r.pos, r.scale, model.tones, members, hits);
      mesh.userData.claims = model.kinds[members[0]] === 1; // chunks never mix layers
      near.push(mesh);
    });
    // Edges: merged LineSegments per source chunk; highlighted links drawn on top.
    const E = Math.min(model.edgeCount, Infinity);
    const per = new Uint32Array(r.chunks);
    let hot = 0;
    const ed = model.edges;
    const valid = (e: number) => ed[e * 2] < n && ed[e * 2 + 1] < n;
    for (let e = 0; e < E; e++) {
      if (!valid(e)) continue;
      if (hits && hits[ed[e * 2]] && hits[ed[e * 2 + 1]]) hot++;
      else per[r.chunk[ed[e * 2]]]++;
    }
    const bufs = Array.from(per, (c) => new Float32Array(c * 6));
    const fill = new Uint32Array(r.chunks);
    const hotPts = new Float32Array(hot * 6);
    let hp = 0;
    const p = r.pos;
    for (let e = 0; e < E; e++) {
      if (!valid(e)) continue;
      const a = ed[e * 2], b = ed[e * 2 + 1];
      let out: Float32Array, o: number;
      if (hits && hits[a] && hits[b]) (out = hotPts), (o = hp), (hp += 6);
      else {
        const c = r.chunk[a];
        out = bufs[c];
        o = fill[c];
        fill[c] += 6;
      }
      out[o] = p[a * 3]; out[o + 1] = p[a * 3 + 1]; out[o + 2] = p[a * 3 + 2];
      out[o + 3] = p[b * 3]; out[o + 4] = p[b * 3 + 1]; out[o + 5] = p[b * 3 + 2];
    }
    // Chunk centres: on big graphs the near view only draws links leaving chunks close to the
    // camera target (links span layers, so their bounding spheres defeat frustum culling).
    const centre = Array.from({ length: r.chunks }, () => new THREE.Vector4());
    for (let i = 0; i < n; i++) centre[r.chunk[i]].add(new THREE.Vector4(p[i * 3], p[i * 3 + 1], p[i * 3 + 2], 1));
    bufs.forEach((b, c) => {
      if (!b.length) return;
      const l = lines(b, edgeMat);
      const w = centre[c];
      l.userData.centre = new THREE.Vector3(w.x / w.w, w.y / w.w, w.z / w.w);
      near.push(l);
    });
    if (hot) {
      const l = lines(hotPts, hotMat);
      l.renderOrder = 2;
      near.push(l);
      far.push(lines(hotPts.slice(), hotMat));
    }
    // Far level of detail: one marker per record cluster plus record-level links.
    const m = r.cRep.length;
    if (n > LOD_MIN && m) {
      const cl = Array.from({ length: m }, (_, k) => k);
      far.push(instanced(r.cPos, r.cScale, r.cTone, cl, null));
      const at = (x: number, o: number) => (x < n ? p[x * 3 + o] : r.cPos[(x - n) * 3 + o]);
      // Edge level of detail: at this distance links are texture, so draw an even sample.
      const fe = r.farEdges;
      const pairs = fe.length / 2;
      const stride = Math.max(1, Math.ceil(pairs / FAR_EDGES));
      const fp = new Float32Array(Math.ceil(pairs / stride) * 6);
      for (let e = 0, k = 0; e < pairs; e += stride, k++)
        for (let d = 0; d < 3; d++) {
          fp[k * 6 + d] = at(fe[e * 2], d);
          fp[k * 6 + 3 + d] = at(fe[e * 2 + 1], d);
        }
      far.push(lines(fp, edgeMat));
      // Pick index for the far view: records' clusters and every non-claim node.
      const ids: number[] = [];
      for (let i = 0; i < n; i++) if (model.kinds[i] !== 1) ids.push(i);
      const fpPos = new Float32Array((ids.length + m) * 3);
      const fpRad = new Float32Array(ids.length + m);
      const map = new Int32Array(ids.length + m);
      ids.forEach((i, k) => {
        fpPos.set(p.subarray(i * 3, i * 3 + 3), k * 3);
        fpRad[k] = 0.5 * r.scale[i];
        map[k] = i;
      });
      for (let c = 0; c < m; c++) {
        const k = ids.length + c;
        fpPos.set(r.cPos.subarray(c * 3, c * 3 + 3), k * 3);
        fpRad[k] = 0.5 * r.cScale[c];
        map[k] = r.cRep[c];
      }
      farGrid = { grid: new SpatialGrid(fpPos, fpRad, ids.length + m), map };
    } else farGrid = null;
    far.forEach((o) => root.add(o));
    near.forEach((o) => root.add(o));
    const rad = new Float32Array(n);
    for (let i = 0; i < n; i++) rad[i] = 0.5 * r.scale[i];
    nearGrid = new SpatialGrid(r.pos, rad, n);
    if (selected !== null) placeMarker(selected);
    stats.nodes = n;
    stats.edges = model.edgeCount;
    stats.rebuildMs += performance.now() - t0;
    if (!stats.firstDrawAt && n) stats.firstDrawAt = performance.now();
    if (model.done && n === model.size) stats.lastDrawAt = performance.now();
  };

  // Far when a node would be under ~1.5 px on screen at the target distance.
  const isFar = () => {
    if (!data || data.n <= LOD_MIN || !data.cRep.length) return false;
    const px = (0.42 * canvas.clientHeight) / (2 * Math.tan(THREE.MathUtils.degToRad(s.camera.fov / 2)) * cam.dist);
    return px < 1.5;
  };
  const setLod = () => {
    const f = isFar();
    stats.lod = f ? "far" : "near";
    far.forEach((o) => (o.visible = f));
    near.forEach((o) => (o.visible = !f || !o.userData.claims));
    // Lines of the near view are only shown near; the far view draws record-level links instead.
    const reach = Math.max(60, cam.dist * 1.5);
    near.forEach((o) => {
      if (!(o as THREE.LineSegments).isLineSegments) return;
      const c = o.userData.centre as THREE.Vector3 | undefined;
      o.visible = !f && (!big || !c || c.distanceTo(cam.target) < reach);
    });
  };

  const placeMarker = (i: number) => {
    if (!data || i >= data.n) return;
    const p = data.pos;
    marker.position.set(p[i * 3], p[i * 3 + 1], p[i * 3 + 2]);
    marker.scale.setScalar(data.scale[i] * 1.9);
    marker.visible = true;
    if (selLines) (s.scene.remove(selLines), selLines.geometry.dispose());
    const nb = model.neighbours(i, 400).filter((j) => j < data!.n);
    const pts = new Float32Array(nb.length * 6);
    nb.forEach((j, k) => {
      pts.set(p.subarray(i * 3, i * 3 + 3), k * 6);
      pts.set(p.subarray(j * 3, j * 3 + 3), k * 6 + 3);
    });
    selLines = lines(pts, selMat);
    selLines.renderOrder = 3;
    s.scene.add(selLines);
  };

  const drift = motion && model.size <= DRIFT_MAX;
  s.start((t) => {
    // While pages stream in, rebuild at most every few rebuild-durations so loading stays responsive.
    let more = false;
    if (pending) {
      const now = performance.now();
      const last = pending.n === model.size && model.done;
      if (last || now - lastRebuild > Math.max(150, lastCost * 4)) {
        const r = pending;
        pending = null;
        rebuild(r);
        lastCost = performance.now() - now;
        lastRebuild = performance.now();
      } else more = true;
    }
    if (drift && !interacted) cam.az = 0.62 + Math.sin(t * 0.00008) * 0.35;
    // Until the user moves the camera, keep the whole graph framed (pages arriving, resize, drift).
    if (!interacted) fit();
    applyCamera();
    setLod();
    stats.frames++;
    stats.drawCalls = s.renderer.info.render.calls;
    return more || (drift && !interacted);
  });

  // Pointer: drag orbits (shift/right-drag pans), wheel zooms, click selects, double-click opens.
  const ray = new THREE.Raycaster();
  const ndc = new THREE.Vector2();
  const pickAt = (x: number, y: number): number | null => {
    if (!data) return null;
    const r = canvas.getBoundingClientRect();
    ndc.set(((x - r.left) / r.width) * 2 - 1, -((y - r.top) / r.height) * 2 + 1);
    ray.setFromCamera(ndc, s.camera);
    const o: [number, number, number] = [ray.ray.origin.x, ray.ray.origin.y, ray.ray.origin.z];
    const d: [number, number, number] = [ray.ray.direction.x, ray.ray.direction.y, ray.ray.direction.z];
    if (isFar() && farGrid) {
      const k = farGrid.grid.pick(o, d);
      return k < 0 ? null : farGrid.map[k];
    }
    const i = nearGrid?.pick(o, d) ?? -1;
    return i < 0 ? null : i;
  };
  let drag: { x: number; y: number; moved: boolean; pan: boolean } | null = null;
  let hoverRaf = 0;
  const touch = () => {
    interacted = true;
    s.kick();
  };
  const onDown = (e: PointerEvent) => {
    drag = { x: e.clientX, y: e.clientY, moved: false, pan: e.shiftKey || e.button === 2 };
    canvas.setPointerCapture?.(e.pointerId);
  };
  const onMove = (e: PointerEvent) => {
    if (drag) {
      const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
      if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
      if (!drag.moved) return;
      drag.x = e.clientX;
      drag.y = e.clientY;
      if (drag.pan) {
        const k = cam.dist / canvas.clientHeight;
        const right = new THREE.Vector3().setFromMatrixColumn(s.camera.matrixWorld, 0);
        const up = new THREE.Vector3().setFromMatrixColumn(s.camera.matrixWorld, 1);
        cam.target.addScaledVector(right, -dx * k).addScaledVector(up, dy * k);
      } else view.orbit(-dx * 0.006, dy * 0.006);
      touch();
      return;
    }
    if (hoverRaf) return;
    const { clientX: x, clientY: y } = e;
    hoverRaf = requestAnimationFrame(() => {
      hoverRaf = 0;
      const i = pickAt(x, y);
      canvas.style.cursor = i === null ? "" : "pointer";
      const r = canvas.getBoundingClientRect();
      h.onHover(i, x - r.left, y - r.top);
    });
  };
  const onUp = (e: PointerEvent) => {
    const d = drag;
    drag = null;
    if (d && !d.moved) {
      const i = pickAt(e.clientX, e.clientY);
      if (i !== null) h.onSelect(i);
    }
  };
  const onDbl = (e: MouseEvent) => {
    const i = pickAt(e.clientX, e.clientY);
    if (i !== null) h.onOpen(i);
  };
  const onWheel = (e: WheelEvent) => {
    e.preventDefault();
    view.zoom(Math.exp(e.deltaY * 0.0015));
  };
  const onLeave = () => h.onHover(null, 0, 0);
  const noMenu = (e: Event) => e.preventDefault();
  canvas.addEventListener("pointerdown", onDown);
  canvas.addEventListener("pointermove", onMove);
  canvas.addEventListener("pointerup", onUp);
  canvas.addEventListener("pointerleave", onLeave);
  canvas.addEventListener("dblclick", onDbl);
  canvas.addEventListener("wheel", onWheel, { passive: false });
  canvas.addEventListener("contextmenu", noMenu);

  const view: GraphView = {
    stats,
    select(i, focus = false) {
      selected = i;
      if (i === null) {
        marker.visible = false;
        if (selLines) (s.scene.remove(selLines), selLines.geometry.dispose(), (selLines = null));
      } else {
        placeMarker(i);
        if (focus && data && i < data.n) {
          cam.target.set(data.pos[i * 3], data.pos[i * 3 + 1], data.pos[i * 3 + 2]);
          // Close enough that the near view (individual claims) shows.
          cam.dist = Math.min(cam.dist, 40);
          interacted = true;
        }
      }
      s.kick();
    },
    orbit(dAz, dEl) {
      cam.az += dAz;
      cam.el = Math.max(-1.45, Math.min(1.45, cam.el + dEl));
      touch();
    },
    zoom(f) {
      cam.dist = Math.max(1.5, Math.min(20000, cam.dist * f));
      touch();
    },
    reset() {
      interacted = false;
      cam.az = 0.62;
      cam.el = 0.28;
      s.kick();
    },
    dispose() {
      unsubscribe();
      worker.terminate();
      cancelAnimationFrame(hoverRaf);
      canvas.removeEventListener("pointerdown", onDown);
      canvas.removeEventListener("pointermove", onMove);
      canvas.removeEventListener("pointerup", onUp);
      canvas.removeEventListener("pointerleave", onLeave);
      canvas.removeEventListener("dblclick", onDbl);
      canvas.removeEventListener("wheel", onWheel);
      canvas.removeEventListener("contextmenu", noMenu);
      clear(near);
      clear(far);
      if (selLines) selLines.geometry.dispose();
      geo.dispose();
      s.dispose();
    },
  };
  // Expose live counters for the measurement harness (web/PERF_GRAPH.md); read-only use.
  (canvas as HTMLCanvasElement & { kizunaGraph?: GraphView }).kizunaGraph = view;
  return view;
}
