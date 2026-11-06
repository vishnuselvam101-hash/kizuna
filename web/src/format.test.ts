import { expect, test } from "vitest";
import { sourceLocation } from "./format";
import { translate, type Lang } from "./i18n";

const tf = (lang: Lang) => ((k: string, v?: Record<string, string | number>) => translate(lang, k, v)) as never;

test("source location shows where the value is, not the extraction pipeline's provenance", () => {
  const loc = { row: 4, page: 1, bbox: [30, 727, 93, 727], layout: "kv", label_basis: "rule", p_correct_bps: 9986, review: "needs_confirmation" };
  expect(sourceLocation(tf("en"), loc)).toBe("page 1, row 4");
  expect(sourceLocation(tf("ja"), loc)).toBe("1ページ目・4行目");
  expect(sourceLocation(tf("en"), { sheet: "Lots", cell: "B7", section: 2 })).toBe("sheet Lots, section 2, cell B7");
  expect(sourceLocation(tf("en"), { bbox_px: [1, 2, 3, 4] })).toBe(translate("en", "evidence.noLocation"));
});
