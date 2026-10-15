// The ⌘K command palette and the ? shortcuts overlay, both built from the keymap and registry.
import { useMemo, useState, type ReactNode } from "react";
import { PALETTE_GROUPS, useActions, type PaletteItem } from "./actions";
import { rank } from "./fuzzy";
import { HELP_ROWS, bindingFor, chordLabel } from "./keymap";

function Scrim({ label, onClose, children }: { label: string; onClose: () => void; children: ReactNode }) {
  return (
    <div className="overlay-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet" role="dialog" aria-label={label} aria-modal="true">
        {children}
      </div>
    </div>
  );
}

export function ShortcutsHelp({ onClose }: { onClose: () => void }) {
  return (
    <Scrim label="Keyboard shortcuts" onClose={onClose}>
      <div className="modal-h">
        <h2>Keyboard shortcuts</h2>
        <button className="iconbtn" aria-label="Close" onClick={onClose}>
          ✕
        </button>
      </div>
      <div className="modal-b">
        <span className="hint">Single keys work when focus is not in a text field.</span>
        <table className="help">
          <tbody>
            {HELP_ROWS.map((row) => {
              const labels = row.actions.map((a) => chordLabel(bindingFor(a).chord));
              const keys = row.range ? `${labels[0]} – ${labels[labels.length - 1]}` : labels.join(" / ");
              return (
                <tr key={row.label}>
                  <td>{row.label}</td>
                  <td>
                    <kbd>{keys}</kbd>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </Scrim>
  );
}

export function Palette({ onClose }: { onClose: () => void }) {
  const registry = useActions();
  const all = useMemo(() => registry.paletteItems(), [registry]);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);

  // Ranked, then grouped: groups in order of their best match (fixed order when not filtering).
  const items = useMemo(() => {
    const ranked = rank(query, all);
    const order = query.trim()
      ? [...new Set(ranked.map((i) => i.group))]
      : PALETTE_GROUPS.filter((g) => ranked.some((i) => i.group === g));
    return order.flatMap((g) => ranked.filter((i) => i.group === g));
  }, [all, query]);
  const sel = Math.min(selected, Math.max(0, items.length - 1));

  const pick = (item: PaletteItem | undefined) => {
    if (!item) return;
    onClose();
    item.run();
  };

  return (
    <Scrim label="Command palette" onClose={onClose}>
      <div className="pal-in">
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
          <circle cx="7" cy="7" r="4.5" />
          <path d="M10.5 10.5L14 14" />
        </svg>
        <input
          autoFocus
          aria-label="Search sessions, worktrees, tasks, views and actions"
          placeholder="Session, worktree, task, or command…"
          autoComplete="off"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setSelected(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setSelected(Math.min(items.length - 1, sel + 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setSelected(Math.max(0, sel - 1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              pick(items[sel]);
            }
          }}
        />
      </div>
      <div className="pal-list" role="listbox" aria-label="Results">
        {items.length === 0 && <p className="empty">Nothing matches “{query}”.</p>}
        {items.map((item, i) => (
          <PaletteRow
            key={`${item.group}:${item.id}`}
            item={item}
            header={i === 0 || items[i - 1].group !== item.group}
            selected={i === sel}
            onPick={() => pick(item)}
          />
        ))}
      </div>
    </Scrim>
  );
}

function PaletteRow(props: { item: PaletteItem; header: boolean; selected: boolean; onPick: () => void }) {
  const { item, header, selected, onPick } = props;
  return (
    <>
      {header && <div className="pal-group">{item.group}</div>}
      <button
        role="option"
        aria-selected={selected}
        className={`pal-item ${selected ? "sel" : ""}`}
        ref={(el) => {
          if (selected) el?.scrollIntoView?.({ block: "nearest" });
        }}
        onClick={onPick}
      >
        <span className="t">
          {item.title}
          {item.detail && <small>{item.detail}</small>}
        </span>
        <span className="k">{item.keys && <kbd>{item.keys}</kbd>}</span>
      </button>
    </>
  );
}
