import { useMemo, useRef, useState, type ReactNode } from "react";
import { useI18n } from "./i18n";

export interface Col<T> {
  key: string;
  label: string;
  /** Sort / filter value. Omit to make the column unsortable. */
  value?: (r: T) => string | number;
  render: (r: T) => ReactNode;
  className?: string;
  srOnlyLabel?: boolean;
}

const ROW_H = 40;
const VIRTUAL_AT = 200;
const VIEW_ROWS = 15;

/**
 * Dense data table: sticky header, sortable columns (aria-sort), text filter, j/k + Enter keyboard
 * navigation, and windowed rendering above 200 rows.
 */
export function DataTable<T>({
  rows,
  cols,
  rowKey,
  label,
  onActivate,
  selectedKey,
  rowClass,
  filterable = true,
}: {
  rows: T[];
  cols: Col<T>[];
  rowKey: (r: T) => string;
  label: string;
  onActivate?: (r: T) => void;
  selectedKey?: string | null;
  rowClass?: (r: T) => string | undefined;
  filterable?: boolean;
}) {
  const { t } = useI18n();
  const [sort, setSort] = useState<{ key: string; dir: 1 | -1 } | null>(null);
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const bodyRef = useRef<HTMLDivElement>(null);

  const view = useMemo(() => {
    const f = q.trim().toLowerCase();
    let r = f ? rows.filter((x) => cols.some((c) => c.value && String(c.value(x)).toLowerCase().includes(f))) : rows;
    const col = sort && cols.find((c) => c.key === sort.key);
    if (col?.value) {
      const v = col.value;
      r = [...r].sort((a, b) => {
        const x = v(a), y = v(b);
        return (typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y))) * sort!.dir;
      });
    }
    return r;
  }, [rows, cols, q, sort]);

  const virtual = view.length > VIRTUAL_AT;
  const start = virtual ? Math.max(0, Math.floor(scrollTop / ROW_H) - 5) : 0;
  const end = virtual ? Math.min(view.length, start + VIEW_ROWS + 10) : view.length;
  const idx = Math.min(active, Math.max(0, view.length - 1));

  function move(to: number) {
    const n = Math.max(0, Math.min(view.length - 1, to));
    setActive(n);
    const el = bodyRef.current;
    if (el && virtual) {
      const top = n * ROW_H;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (top > el.scrollTop + (VIEW_ROWS - 1) * ROW_H) el.scrollTop = top - (VIEW_ROWS - 1) * ROW_H;
    } else {
      el?.querySelector<HTMLElement>(`[data-row="${n}"]`)?.scrollIntoView?.({ block: "nearest" });
    }
  }

  function onKey(e: React.KeyboardEvent) {
    if ((e.target as HTMLElement).closest("input,select,textarea,button:not(.th-btn)") && e.target !== e.currentTarget) return;
    if (e.key === "j" || e.key === "ArrowDown") {
      e.preventDefault();
      move(idx + 1);
    } else if (e.key === "k" || e.key === "ArrowUp") {
      e.preventDefault();
      move(idx - 1);
    } else if (e.key === "Enter" && onActivate && view[idx]) {
      e.preventDefault();
      onActivate(view[idx]);
    }
  }

  return (
    <div className="dt">
      {filterable && rows.length > 5 && (
        <label className="dt-filter">
          <span className="sr-only">{t("table.filter", { label })}</span>
          <input type="search" placeholder={t("table.filterPlaceholder")} value={q} onChange={(e) => setQ(e.target.value)} />
          <span className="muted num">{t("table.count", { n: view.length, total: rows.length })}</span>
        </label>
      )}
      <div
        className={`table-wrap${virtual ? " virtual" : ""}`}
        ref={bodyRef}
        style={virtual ? { maxHeight: VIEW_ROWS * ROW_H + ROW_H } : undefined}
        onScroll={virtual ? (e) => setScrollTop(e.currentTarget.scrollTop) : undefined}
        tabIndex={0}
        role="region"
        aria-label={t("table.keyboardHint", { label })}
        onKeyDown={onKey}
        title={t("table.shortcuts")}
      >
        <table aria-label={label} aria-rowcount={virtual ? view.length + 1 : undefined}>
          <thead>
            <tr>
              {cols.map((c) => {
                const dir = sort?.key === c.key ? (sort.dir === 1 ? "ascending" : "descending") : "none";
                return (
                  <th key={c.key} scope="col" aria-sort={c.value ? dir : undefined}>
                    {c.value ? (
                      <button
                        className="th-btn"
                        onClick={() => setSort(sort?.key === c.key ? { key: c.key, dir: sort.dir === 1 ? -1 : 1 } : { key: c.key, dir: 1 })}
                      >
                        {c.label}
                        <span aria-hidden="true" className="sort-ind">
                          {dir === "ascending" ? "↑" : dir === "descending" ? "↓" : ""}
                        </span>
                      </button>
                    ) : c.srOnlyLabel ? (
                      <span className="sr-only">{c.label}</span>
                    ) : (
                      c.label
                    )}
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {virtual && start > 0 && <tr aria-hidden="true" style={{ height: start * ROW_H }} />}
            {view.slice(start, end).map((r, i) => {
              const n = start + i;
              const k = rowKey(r);
              const cls = [n === idx ? "active" : "", selectedKey === k ? "selected" : "", onActivate ? "clickable" : "", rowClass?.(r) ?? ""]
                .filter(Boolean)
                .join(" ");
              return (
                <tr
                  key={k}
                  data-row={n}
                  aria-rowindex={virtual ? n + 2 : undefined}
                  aria-current={selectedKey === k ? "true" : undefined}
                  className={cls || undefined}
                  style={virtual ? { height: ROW_H } : undefined}
                  onClick={
                    onActivate
                      ? () => {
                          setActive(n);
                          onActivate(r);
                        }
                      : undefined
                  }
                >
                  {cols.map((c) => (
                    <td key={c.key} className={c.className}>
                      {c.render(r)}
                    </td>
                  ))}
                </tr>
              );
            })}
            {virtual && end < view.length && <tr aria-hidden="true" style={{ height: (view.length - end) * ROW_H }} />}
          </tbody>
        </table>
      </div>
    </div>
  );
}
