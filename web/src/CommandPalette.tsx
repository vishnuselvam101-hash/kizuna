import { useEffect, useMemo, useRef, useState } from "react";
import { useI18n } from "./i18n";

export interface Command {
  id: string;
  label: string;
  group: string;
  run: () => void;
}

/** ⌘K / Ctrl+K palette: filter, ↑/↓, Enter, Esc. Focus returns to the opener on close. */
export function CommandPalette({ commands }: { commands: Command[] }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");
  const [i, setI] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const opener = useRef<Element | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        opener.current = document.activeElement;
        setOpen((o) => !o);
      }
    };
    const onOpen = () => {
      opener.current = document.activeElement;
      setOpen(true);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("kizuna:palette", onOpen);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("kizuna:palette", onOpen);
    };
  }, []);
  useEffect(() => {
    if (open) {
      setQ("");
      setI(0);
      input.current?.focus();
    } else (opener.current as HTMLElement | null)?.focus?.();
  }, [open]);

  const list = useMemo(() => {
    const f = q.trim().toLowerCase();
    return f ? commands.filter((c) => c.label.toLowerCase().includes(f) || c.group.toLowerCase().includes(f)) : commands;
  }, [q, commands]);

  if (!open) return null;
  const run = (c?: Command) => {
    if (!c) return;
    setOpen(false);
    c.run();
  };
  return (
    <div className="palette-backdrop" onMouseDown={() => setOpen(false)}>
      <div className="palette glass" role="dialog" aria-modal="true" aria-label={t("palette.title")} onMouseDown={(e) => e.stopPropagation()}>
        <input
          ref={input}
          type="text"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={list[i] ? `cmd-${list[i].id}` : undefined}
          aria-label={t("palette.title")}
          placeholder={t("palette.placeholder")}
          value={q}
          onChange={(e) => (setQ(e.target.value), setI(0))}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") (e.preventDefault(), setI((n) => Math.min(list.length - 1, n + 1)));
            else if (e.key === "ArrowUp") (e.preventDefault(), setI((n) => Math.max(0, n - 1)));
            else if (e.key === "Enter") (e.preventDefault(), run(list[i]));
            else if (e.key === "Escape") setOpen(false);
          }}
        />
        <ul id="palette-list" role="listbox" aria-label={t("palette.results")}>
          {list.map((c, n) => (
            <li key={c.id} id={`cmd-${c.id}`} role="option" aria-selected={n === i} onMouseEnter={() => setI(n)} onClick={() => run(c)}>
              <span>{c.label}</span>
              <span className="muted">{c.group}</span>
            </li>
          ))}
          {list.length === 0 && <li className="muted" role="option" aria-selected="false" aria-disabled="true">{t("palette.none")}</li>}
        </ul>
      </div>
    </div>
  );
}
