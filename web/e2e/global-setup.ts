// Renders the Japanese PDF documents with Chromium (real CJK text with ToUnicode maps, as office tools produce):
//   検査成績書_JA-BRK-1001.pdf  inspection certificate (rev B, SUS304, measured length, 判定 合格)
//   ミルシート_JA-BRK-1001.pdf   mill sheet from the material supplier — states SUS316 (conflicts with the spec)
//   スキャン図面_JA-BRK-1001.pdf  image-only PDF (a JPEG of a title block): needs OCR
//   補足仕様_JA-BRK-1001.pdf    revision C spec (used to supersede evidence)
//   検査成績書_改竄.pdf           inspection certificate with an embedded /JS action → must be quarantined
import { chromium } from "@playwright/test";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const FONT = `font-family: "Noto Sans CJK JP", "Noto Sans JP", sans-serif;`;
const page = (title: string, rows: [string, string][], extra = "") => `<!doctype html><html lang="ja"><meta charset="utf-8">
<style>body{${FONT} margin:40px; font-size:14px} h1{font-size:22px; letter-spacing:.3em} p{margin:6px 0}</style>
<h1>${title}</h1>${rows.map(([k, v]) => `<p>${k}：${v}</p>`).join("")}${extra}</html>`;

/** Minimal PDF wrapping one baseline JPEG as a full page (what a document scanner produces). */
function jpegPdf(jpeg: Buffer, w: number, h: number): Buffer {
  const parts: Buffer[] = [];
  const offsets: number[] = [];
  let len = 0;
  const push = (b: Buffer | string) => {
    const buf = typeof b === "string" ? Buffer.from(b, "latin1") : b;
    parts.push(buf);
    len += buf.length;
  };
  const obj = (n: number, body: (Buffer | string)[]) => {
    offsets[n] = len;
    push(`${n} 0 obj\n`);
    body.forEach(push);
    push("\nendobj\n");
  };
  const pw = Math.round((w * 72) / 150), ph = Math.round((h * 72) / 150);
  const content = `q ${pw} 0 0 ${ph} 0 0 cm /Im0 Do Q`;
  push("%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");
  obj(1, ["<< /Type /Catalog /Pages 2 0 R >>"]);
  obj(2, ["<< /Type /Pages /Kids [3 0 R] /Count 1 >>"]);
  obj(3, [`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${pw} ${ph}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>`]);
  obj(4, [`<< /Type /XObject /Subtype /Image /Width ${w} /Height ${h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length ${jpeg.length} >>\nstream\n`, jpeg, "\nendstream"]);
  obj(5, [`<< /Length ${content.length} >>\nstream\n${content}\nendstream`]);
  const xref = len;
  push(`xref\n0 6\n0000000000 65535 f \n${offsets.slice(1).map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}`);
  push(`trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`);
  return Buffer.concat(parts);
}

export default async function globalSetup() {
  const dir = join(process.env.E2E_DATA ?? "e2e/results/data", "ja");
  mkdirSync(dir, { recursive: true });
  if (existsSync(join(dir, "ミルシート_JA-BRK-1001.pdf"))) return;
  const browser = await chromium.launch();
  const p = await browser.newPage();
  const pdf = async (name: string, html: string) => {
    await p.setContent(html);
    writeFileSync(join(dir, name), await p.pdf({ format: "A4" }));
  };
  await pdf(
    "検査成績書_JA-BRK-1001.pdf",
    page("検査成績書", [
      ["品番", "JA-BRK-1001"],
      ["改訂", "B"],
      ["材質", "SUS304"],
      ["ロット", "L-2401"],
      ["長さ (mm)", "120.0"],
      ["判定", "合格"],
      ["仕入先", "東邦精工株式会社"],
      ["有効期限", "2031-03-31"],
    ]),
  );
  await pdf(
    "ミルシート_JA-BRK-1001.pdf",
    page("ミルシート（鋼材検査証明書）", [
      ["品番", "JA-BRK-1001"],
      ["改訂", "B"],
      ["材質", "SUS316"],
      ["引張強さ (MPa)", "580"],
      ["硬さ (HV)", "175"],
      ["仕入先", "北陸金属工業"],
    ]),
  );
  await pdf(
    "補足仕様_JA-BRK-1001.pdf",
    page("仕様書（改訂C）", [
      ["品番", "JA-BRK-1001"],
      ["改訂", "C"],
      ["材質", "SUS304"],
      ["長さ (mm)", "125.0"],
      ["承認", "Released"],
    ]),
  );
  // Scanned drawing: render a title block, photograph it as JPEG, wrap it in an image-only PDF.
  await p.setViewportSize({ width: 1200, height: 700 });
  await p.setContent(`<!doctype html><html lang="ja"><meta charset="utf-8"><style>body{${FONT} margin:0; background:#fff}
  .tb{margin:60px; border:3px solid #000; padding:30px; font-size:40px; line-height:1.6}</style>
  <div class="tb"><div>図番：D-JA-5002</div><div>品番：JA-BRK-1001</div><div>改訂：B</div><div>材質：SUS304</div></div></html>`);
  const jpeg = await p.screenshot({ type: "jpeg", quality: 92, clip: { x: 0, y: 0, width: 1200, height: 700 } });
  writeFileSync(join(dir, "スキャン図面_JA-BRK-1001.pdf"), jpegPdf(jpeg, 1200, 700));
  await browser.close();
  // Hostile document: a real-looking certificate with an OpenAction JavaScript (static scan → quarantine).
  writeFileSync(
    join(dir, "検査成績書_改竄.pdf"),
    Buffer.from("%PDF-1.7\n1 0 obj << /Type /Catalog /OpenAction << /S /JavaScript /JS (app.alert('検査合格')) >> >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n", "utf8"),
  );
}
