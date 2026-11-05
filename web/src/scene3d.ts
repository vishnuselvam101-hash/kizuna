// Lazy-loaded WebGL scenes (three.js). Imported only by Scene.tsx via dynamic import, so the
// app shell bundle never contains three.js. Rendering pauses when the tab is hidden or the
// canvas is off-screen; transform-only motion, no per-frame allocation.
import * as THREE from "three";
import type { Graph } from "./graph";

export type Dispose = () => void;

function token(name: string, fallback: string): THREE.Color {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return new THREE.Color(v || fallback);
}

function setup(canvas: HTMLCanvasElement) {
  const renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true, powerPreference: "low-power" });
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
  const ro = new ResizeObserver(resize);
  ro.observe(canvas);
  scene.add(new THREE.AmbientLight(0xffffff, 0.35));
  const key = new THREE.DirectionalLight(0xffffff, 1.5);
  key.position.set(4, 6, 8);
  const rim = new THREE.DirectionalLight(token("--info", "#3fd0c9"), 0.5);
  rim.position.set(-6, -2, -4);
  scene.add(key, rim);

  let visible = true;
  let raf = 0;
  let frame: (t: number) => void = () => {};
  const loop = (t: number) => {
    frame(t);
    renderer.render(scene, camera);
    raf = visible && !document.hidden ? requestAnimationFrame(loop) : 0;
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
    start(f: (t: number) => void) {
      frame = f;
      kick();
    },
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
  });
  return () => {
    window.removeEventListener("pointermove", onMove);
    s.dispose();
  };
}

const SWING = 0.5;
const LAYER_X: Record<string, number> = { document: -8, claim: -3, subject: 2.5, finding: 7.5, decision: 7.5 };

/** Evidence graph as an orbiting faceted structure; click a node to pick it. */
export function mountGraph(canvas: HTMLCanvasElement, g: Graph, onPick: (id: string) => void, highlight: ReadonlySet<string> = new Set()): Dispose {
  const s = setup(canvas);
  s.camera.position.set(0, 2, 15);
  s.camera.lookAt(0, 0, 0);
  const root = new THREE.Group();
  s.scene.add(root);
  const tones: Record<string, THREE.Color> = {
    crit: token("--crit", "#ff4d2e"),
    ok: token("--gold", "#d8b26a"),
    warn: token("--warn", "#e7a33e"),
    info: token("--info", "#3fd0c9"),
    base: token("--fg-muted", "#8a95ad"),
  };
  const geo = new THREE.OctahedronGeometry(0.42, 0);
  const pos = new Map<string, THREE.Vector3>();
  const byLayer = new Map<string, number>();
  const counts = new Map<string, number>();
  g.nodes.forEach((n) => counts.set(n.kind, (counts.get(n.kind) ?? 0) + 1));
  const meshes: THREE.Mesh[] = [];
  for (const n of g.nodes) {
    const i = byLayer.get(n.kind) ?? 0;
    byLayer.set(n.kind, i + 1);
    const c = counts.get(n.kind)!;
    const a = (i / c) * Math.PI * 2 + (n.kind === "finding" ? Math.PI / c : 0);
    const r = Math.min(6, 0.6 + Math.sqrt(c) * 0.9);
    const p = new THREE.Vector3(LAYER_X[n.kind] ?? 0, Math.cos(a) * r, Math.sin(a) * r);
    pos.set(n.id, p);
    const col = tones[n.tone] ?? tones.base;
    const hit = highlight.has(n.id);
    const dim = highlight.size > 0 && !hit;
    const m = new THREE.Mesh(
      geo,
      new THREE.MeshStandardMaterial({
        color: col,
        flatShading: true,
        roughness: 0.4,
        metalness: 0.3,
        // What-if highlight: affected nodes glow, the rest of the structure recedes.
        emissive: hit ? col : new THREE.Color(0, 0, 0),
        emissiveIntensity: hit ? 0.65 : 0,
        transparent: dim,
        opacity: dim ? 0.28 : 1,
      }),
    );
    m.position.copy(p);
    m.scale.setScalar((n.kind === "decision" || n.kind === "finding" ? 1.4 : n.kind === "subject" ? 1.1 : 0.8) * (hit ? 1.3 : 1));
    m.userData.id = n.id;
    meshes.push(m);
    root.add(m);
  }
  // Centre the structure and keep it framed: the camera distance follows the canvas aspect so
  // nodes are never clipped at narrow widths or left floating off-centre on wide ones.
  const box = new THREE.Box3();
  pos.forEach((p) => box.expandByPoint(p));
  const centre = box.getCenter(new THREE.Vector3());
  pos.forEach((p) => p.sub(centre));
  meshes.forEach((m) => m.position.sub(centre));
  const size = box.getSize(new THREE.Vector3());
  // The group swings about y, so horizontal reach is the x–z radius, not just the x extent.
  let reach = 0;
  pos.forEach((p) => (reach = Math.max(reach, Math.hypot(p.x, p.z))));
  reach += 0.8; // node size + breathing room
  const halfH = size.y / 2 + 1;
  const tanHalf = Math.tan(THREE.MathUtils.degToRad(s.camera.fov / 2));
  const frameCamera = () => {
    // Nodes lie on rings in the y–z plane: the top/bottom extremes sit near z = 0, so only a
    // small depth allowance is needed for the nearer side nodes.
    // Width: a node at reach r swung by up to SWING rad sits at x = r·cos, z = r·sin toward the camera.
    const dW = reach * (Math.cos(SWING) / (tanHalf * s.camera.aspect) + Math.sin(SWING));
    const d = Math.max(halfH / tanHalf + 1.5, dW, 6);
    s.camera.position.set(0, d * 0.12, d);
    s.camera.lookAt(0, 0, 0);
  };
  frameCamera();
  const pts: number[] = [];
  const hot: number[] = [];
  for (const [a, b] of g.edges) {
    const pa = pos.get(a), pb = pos.get(b);
    if (pa && pb) (highlight.has(a) && highlight.has(b) ? hot : pts).push(pa.x, pa.y, pa.z, pb.x, pb.y, pb.z);
  }
  const lg = new THREE.BufferGeometry();
  lg.setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
  root.add(new THREE.LineSegments(lg, new THREE.LineBasicMaterial({ color: tones.info, transparent: true, opacity: highlight.size ? 0.12 : 0.28 })));
  if (hot.length) {
    const hg = new THREE.BufferGeometry();
    hg.setAttribute("position", new THREE.Float32BufferAttribute(hot, 3));
    root.add(new THREE.LineSegments(hg, new THREE.LineBasicMaterial({ color: tones.crit, transparent: true, opacity: 0.9 })));
  }

  const ray = new THREE.Raycaster();
  const ndc = new THREE.Vector2();
  const onClick = (e: MouseEvent) => {
    const r = canvas.getBoundingClientRect();
    ndc.set(((e.clientX - r.left) / r.width) * 2 - 1, -((e.clientY - r.top) / r.height) * 2 + 1);
    ray.setFromCamera(ndc, s.camera);
    const hit = ray.intersectObjects(meshes)[0];
    if (hit) onPick(hit.object.userData.id as string);
  };
  const onMove = (e: MouseEvent) => {
    const r = canvas.getBoundingClientRect();
    ndc.set(((e.clientX - r.left) / r.width) * 2 - 1, -((e.clientY - r.top) / r.height) * 2 + 1);
    ray.setFromCamera(ndc, s.camera);
    canvas.style.cursor = ray.intersectObjects(meshes).length ? "pointer" : "";
  };
  canvas.addEventListener("click", onClick);
  canvas.addEventListener("mousemove", onMove);
  s.start((t) => {
    frameCamera();
    root.rotation.y = Math.sin(t * 0.00008) * SWING;
    root.rotation.x = Math.sin(t * 0.00005) * 0.12;
  });
  return () => {
    canvas.removeEventListener("click", onClick);
    canvas.removeEventListener("mousemove", onMove);
    s.dispose();
  };
}
