import { useCallback, useEffect, useMemo, useReducer, useState } from "react";
import {
  ActionsContext,
  createActionRegistry,
  useHandler,
  usePaletteSourceIn,
  type ActionRegistry,
  type PaletteItem,
} from "./actions";
import { BINDINGS, inTextField, keysLabel, matchesChord, type ActionId } from "./keymap";
import { Palette, ShortcutsHelp } from "./Overlays";
import type { CoreConnection } from "./core";
import { dismissToast, initialState, reduce, type RepositoryView, type State } from "./state";
import type { Event } from "./schema/Event";

type View = "dashboard" | "session" | "worktrees" | "tasks";
const VIEWS: [View, string, ActionId][] = [
  ["dashboard", "Dashboard", "view.dashboard"],
  ["session", "Session", "view.session"],
  ["worktrees", "Worktrees", "view.worktrees"],
  ["tasks", "Tasks", "view.tasks"],
];

const TOAST_MS = 4000;

type Action = { kind: "event"; event: Event } | { kind: "dismiss"; id: number };
function appReducer(state: State, action: Action): State {
  return action.kind === "event" ? reduce(state, action.event) : dismissToast(state, action.id);
}

// UI preferences (dock, sidebar, last view, theme) live in renderer local storage;
// everything else comes from the core.
const PREFS_KEY = "switchyard.ui";
interface Prefs {
  view: View;
  dockCollapsed: boolean;
  sidebarCollapsed: boolean;
  theme: "dark" | "light" | null;
}
const defaultPrefs: Prefs = { view: "dashboard", dockCollapsed: false, sidebarCollapsed: false, theme: null };
function loadPrefs(): Prefs {
  try {
    return { ...defaultPrefs, ...JSON.parse(localStorage.getItem(PREFS_KEY) ?? "{}") };
  } catch {
    return defaultPrefs;
  }
}
function savePrefs(prefs: Prefs) {
  try {
    localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
  } catch {
    /* preferences are a convenience */
  }
}

export interface AppProps {
  core: CoreConnection;
  /** Native folder picker; resolves to null when cancelled. */
  pickFolder: () => Promise<string | null>;
  /** Where shortcut and palette handlers are registered; tests pass one to observe actions. */
  actions?: ActionRegistry;
}

type Overlay = "palette" | "help" | null;

export function App({ core, pickFolder, actions }: AppProps) {
  const registry = useMemo(() => actions ?? createActionRegistry(), [actions]);
  const [overlay, setOverlay] = useState<Overlay>(null);
  const [state, dispatch] = useReducer(appReducer, initialState);
  const [prefs, setPrefs] = useState(loadPrefs);
  const [sidebarOpen, setSidebarOpen] = useState(false); // overlay below 860 px

  const updatePrefs = useCallback((patch: Partial<Prefs>) => {
    setPrefs((p) => {
      const next = { ...p, ...patch };
      savePrefs(next);
      return next;
    });
  }, []);

  useEffect(() => {
    const unsubscribe = core.subscribe((event) => dispatch({ kind: "event", event }));
    core.send({ type: "attach" });
    return unsubscribe;
  }, [core]);

  useEffect(() => {
    if (prefs.theme) document.documentElement.dataset.theme = prefs.theme;
    else delete document.documentElement.dataset.theme;
  }, [prefs.theme]);

  const latestToast = state.toasts[state.toasts.length - 1];
  useEffect(() => {
    if (!latestToast) return;
    const id = latestToast.id;
    const timer = setTimeout(() => dispatch({ kind: "dismiss", id }), TOAST_MS);
    return () => clearTimeout(timer);
  }, [latestToast]);

  const toggleSidebar = useCallback(() => {
    if (window.matchMedia?.("(max-width: 860px)").matches) setSidebarOpen((o) => !o);
    else updatePrefs({ sidebarCollapsed: !prefs.sidebarCollapsed });
  }, [prefs.sidebarCollapsed, updatePrefs]);

  const toggleTheme = useCallback(() => {
    const dark = prefs.theme
      ? prefs.theme === "dark"
      : !window.matchMedia?.("(prefers-color-scheme: light)").matches;
    updatePrefs({ theme: dark ? "light" : "dark" });
  }, [prefs.theme, updatePrefs]);

  const toggleDock = useCallback(
    () => updatePrefs({ dockCollapsed: !prefs.dockCollapsed }),
    [prefs.dockCollapsed, updatePrefs],
  );

  useHandler(registry, "palette.toggle", () => setOverlay((o) => (o === "palette" ? null : "palette")));
  useHandler(registry, "help.open", () => setOverlay("help"));
  useHandler(registry, "sidebar.toggle", toggleSidebar);
  useHandler(registry, "dock.toggle", toggleDock);
  useHandler(registry, "view.dashboard", () => updatePrefs({ view: "dashboard" }));
  useHandler(registry, "view.session", () => updatePrefs({ view: "session" }));
  useHandler(registry, "view.worktrees", () => updatePrefs({ view: "worktrees" }));
  useHandler(registry, "view.tasks", () => updatePrefs({ view: "tasks" }));
  // Esc is layered: close an overlay, else cancel an inline confirm, else interrupt the agent.
  useHandler(registry, "escape", () => {
    if (overlay) return setOverlay(null);
    if (sidebarOpen) return setSidebarOpen(false);
    return registry.run("confirm.cancel") || registry.run("session.interrupt");
  });

  usePaletteSourceIn(registry, () => {
    const items: PaletteItem[] = [];
    for (const r of state.repositories)
      for (const w of r.worktrees) {
        const branch = w.branch ?? "(detached)";
        items.push({
          id: w.path,
          group: "Worktrees",
          title: `${r.name}/${branch}`,
          keywords: [branch],
          detail: w.primary ? "primary" : "free",
          run: () => updatePrefs({ view: "worktrees" }),
        });
      }
    for (const [view, label, action] of VIEWS)
      items.push({ id: view, group: "Views", title: label, keys: keysLabel(action), run: () => updatePrefs({ view }) });
    for (const binding of BINDINGS)
      if (binding.title && registry.has(binding.action))
        items.push({
          id: binding.action,
          group: "Actions",
          title: binding.title,
          keys: keysLabel(binding.action),
          run: () => registry.run(binding.action),
        });
    items.push({ id: "theme", group: "Actions", title: "Toggle theme", run: toggleTheme });
    items.push({ id: "addRepository", group: "Actions", title: "Add repository", run: addRepository });
    return items;
  });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const binding = BINDINGS.find((b) => matchesChord(e, b.chord));
      if (!binding || binding.scope === "composer" || binding.scope === "terminal") return;
      // Single keys never fire while typing or under an overlay.
      if (binding.scope === "outsideText" && (inTextField(e.target) || overlay)) return;
      if (binding.scope === "global" && binding.action !== "escape") e.preventDefault();
      if (registry.run(binding.action) && binding.action === "escape") e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [registry, overlay]);

  const addRepository = async () => {
    const path = await pickFolder();
    if (path) core.send({ type: "addRepository", path });
  };
  const closeOverlay = () => setOverlay(null);

  const worktreeCount = state.repositories.reduce((n, r) => n + r.worktrees.length, 0);
  const appClass = [
    "app",
    prefs.sidebarCollapsed ? "sidebar-collapsed" : "",
    sidebarOpen ? "sidebar-open" : "",
  ].join(" ");

  return (
    <ActionsContext.Provider value={registry}>
      <div className={appClass}>
        <header className="topbar">
          <button className="iconbtn" title="Toggle sidebar (⌘B)" aria-label="Toggle sidebar" onClick={toggleSidebar}>
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.4">
              <rect x="1.5" y="2.5" width="13" height="11" rx="2" />
              <path d="M6 2.5v11" />
            </svg>
          </button>
          <div className="brand">
            <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
              <path d="M3 15h5c3 0 4-2 5-4s2-4 5-4h-1" />
              <path d="M3 7h5c3 0 4 2 5 4s2 4 5 4h-1" />
              <circle cx="3" cy="7" r="1.3" fill="currentColor" stroke="none" />
              <circle cx="3" cy="15" r="1.3" fill="currentColor" stroke="none" />
            </svg>
            <span className="lbl">Switchyard</span>
          </div>
          <nav className="views" aria-label="Views">
            {VIEWS.map(([view, label, action]) => (
              <button
                key={view}
                className={`tab ${prefs.view === view ? "active" : ""}`}
                title={`${label} (${keysLabel(action)})`}
                aria-label={label}
                aria-current={prefs.view === view ? "page" : undefined}
                onClick={() => updatePrefs({ view })}
              >
                <ViewIcon view={view} />
                <span className="lbl">{label}</span>
                {view === "worktrees" && worktreeCount > 0 && <span className="n">{worktreeCount}</span>}
              </button>
            ))}
          </nav>
          <div className="spacer" />
          <button className="palette-btn" aria-label="Command palette" onClick={() => registry.run("palette.toggle")}>
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
              <circle cx="7" cy="7" r="4.5" />
              <path d="M10.5 10.5L14 14" />
            </svg>
            <span className="q">Jump to session, worktree, task…</span>
            <kbd>{keysLabel("palette.toggle")}</kbd>
          </button>
          <button
            className="iconbtn"
            title="Keyboard shortcuts (?)"
            aria-label="Keyboard shortcuts"
            onClick={() => registry.run("help.open")}
          >
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
              <circle cx="8" cy="8" r="6.5" />
              <path d="M6 6.5a2 2 0 113 1.7c-.7.4-1 .8-1 1.5M8 12h.01" />
            </svg>
          </button>
          <button className="iconbtn" title="Toggle theme" aria-label="Toggle theme" onClick={toggleTheme}>
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5">
              <circle cx="8" cy="8" r="5.5" />
              <path d="M8 2.5v11a5.5 5.5 0 000-11z" fill="currentColor" stroke="none" />
            </svg>
          </button>
          <button className="btn primary" disabled title="Arrives with sessions">
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.8">
              <path d="M8 3v10M3 8h10" />
            </svg>
            <span className="lbl">New session</span>
          </button>
        </header>

        <aside className="sidebar" aria-label="Repositories">
          <div className="sidebar-head">
            <h2>Repositories</h2>
            <span className="spacer" />
            <button className="btn ghost sm" title="Open a repository" onClick={addRepository}>
              + Repo
            </button>
          </div>
          <div className="sidebar-list">
            {state.repositories.length === 0 ? (
              <p className="empty">No repositories yet. Add one with + Repo.</p>
            ) : (
              state.repositories.map((r) => <RepositoryGroup key={r.id} repository={r} />)
            )}
          </div>
          <div className="sidebar-foot">
            <span>
              {state.repositories.length} repos · {worktreeCount} worktrees
            </span>
            <span>Local</span>
          </div>
        </aside>
        {sidebarOpen && <div className="scrim" onClick={() => setSidebarOpen(false)} />}

        <main className="stage">
          <div className="page">
            <div className="pagehead">
              <h1>{VIEWS.find(([v]) => v === prefs.view)?.[1]}</h1>
            </div>
            <p className="empty">Nothing here yet.</p>
          </div>
        </main>

        <section className={`dock ${prefs.dockCollapsed ? "collapsed" : ""}`} aria-label="Terminals">
          <div className="dock-head">
            <div className="tabs" />
            <button className="btn ghost sm" onClick={toggleDock}>
              {prefs.dockCollapsed ? "Show" : "Hide"} <kbd>`</kbd>
            </button>
          </div>
          <div className="term">
            <p className="empty">No terminals open.</p>
          </div>
        </section>

        <footer className="statusbar">
          <span className="i">
            <i className="dot running" />0 running
          </span>
          <span className="i">
            <i className="dot waiting" />0 waiting
          </span>
          <div className="r">
            <span className="i hide-sm">{state.repositories.length} repos</span>
            <span className="i">$0.00 today</span>
          </div>
        </footer>

        <div className="toasts" role="status" aria-live="polite">
          {state.toasts.map((t) => (
            <div key={t.id} className={`toast ${t.tone}`}>
              <i />
              <span className="tx">{t.text}</span>
              <button className="x" aria-label="Dismiss" onClick={() => dispatch({ kind: "dismiss", id: t.id })}>
                ×
              </button>
            </div>
          ))}
        </div>
        {overlay === "palette" && <Palette onClose={closeOverlay} />}
        {overlay === "help" && <ShortcutsHelp onClose={closeOverlay} />}
      </div>
    </ActionsContext.Provider>
  );
}

function RepositoryGroup({ repository }: { repository: RepositoryView }) {
  const [collapsed, setCollapsed] = useState(false);
  return (
    <div className={`repo ${collapsed ? "collapsed" : ""}`} role="group" aria-label={repository.name}>
      <button className="repo-row" title={repository.path} onClick={() => setCollapsed((c) => !c)}>
        <svg className="chev" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
          <path d="M3 4.5l3 3 3-3" />
        </svg>
        <span className="name">{repository.name}</span>
        <span className="n">base {repository.baseBranch}</span>
      </button>
      <ul className="worktrees">
        {repository.worktrees.map((w) => (
          <li key={w.path} className="worktree" title={w.path}>
            <span className="dot" />
            <span className="branch">
              {w.branch ?? "(detached)"}
              {w.primary && <span className="tag">primary</span>}
            </span>
            <span className="meta">free</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function ViewIcon({ view }: { view: View }) {
  const paths: Record<View, JSX.Element> = {
    dashboard: <path d="M2.5 2.5h4.5v5H2.5zM9 2.5h4.5v3H9zM9 7.5h4.5v6H9zM2.5 9.5h4.5v4H2.5z" />,
    session: <path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z" />,
    worktrees: (
      <>
        <circle cx="4.5" cy="3.5" r="1.5" />
        <circle cx="4.5" cy="12.5" r="1.5" />
        <circle cx="11.5" cy="6" r="1.5" />
        <path d="M4.5 5v6M11.5 7.5c0 2.5-3 2.5-7 3.5" />
      </>
    ),
    tasks: <path d="M3 4l1.5 1.5L7 3M3 10l1.5 1.5L7 9M9 4.5h4.5M9 10.5h4.5" />,
  };
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.4">
      {paths[view]}
    </svg>
  );
}
