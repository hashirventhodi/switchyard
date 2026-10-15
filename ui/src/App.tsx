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
import { Dock } from "./Dock";
import { shellStreams, type ShellStreams } from "./shellStreams";
import { xtermTerminal, type CreateTerminal } from "./terminal";
import { NewSession, type NewSessionPreset } from "./NewSession";
import { SessionView, type Draft } from "./SessionView";
import { NewWorktree, WorktreesView } from "./WorktreesView";
import { NewTask, TasksView, type Linking } from "./TasksView";
import { BehindBadge, BranchPane, branchOf } from "./History";
import { SetupPanel, type OpenLink } from "./Setup";
import { PrBadge, PullRequestActions } from "./PullRequest";
import { SpendToday } from "./spend";
import type { CoreConnection } from "./core";
import { Dashboard } from "./Dashboard";
import { Inbox } from "./Inbox";
import {
  dismissToast,
  initialState,
  reduce,
  resultToast,
  sessionOn,
  taskById,
  waitingSessions,
  type PendingRequest,
  type RepositoryView,
  type State,
  type Toast,
} from "./state";
import { AGENT_SHORT, STATUS_LABEL, Status } from "./status";
import type { Command } from "./schema/Command";
import type { Event } from "./schema/Event";
import type { Verdict } from "./schema/Verdict";

type View = "dashboard" | "session" | "worktrees" | "tasks";
const VIEWS: [View, string, ActionId][] = [
  ["dashboard", "Dashboard", "view.dashboard"],
  ["session", "Session", "view.session"],
  ["worktrees", "Worktrees", "view.worktrees"],
  ["tasks", "Tasks", "view.tasks"],
];

type Action = { kind: "event"; event: Event } | { kind: "dismiss"; id: number } | { kind: "result"; text: string };
function appReducer(state: State, action: Action): State {
  switch (action.kind) {
    case "event":
      return reduce(state, action.event);
    case "dismiss":
      return dismissToast(state, action.id);
    case "result":
      return resultToast(state, action.text);
  }
}

// UI preferences (dock, sidebar, last view, theme) live in renderer local storage;
// everything else comes from the core.
const PREFS_KEY = "switchyard.ui";
interface Prefs {
  view: View;
  dockCollapsed: boolean;
  sidebarCollapsed: boolean;
  theme: "dark" | "light" | null;
  /** The Agent log shows raw adapter events (debug). */
  rawEvents: boolean;
  /** The session's review pane is hidden (⌘⇧D). */
  reviewHidden: boolean;
}
const defaultPrefs: Prefs = {
  view: "dashboard",
  dockCollapsed: false,
  sidebarCollapsed: false,
  theme: null,
  rawEvents: false,
  reviewHidden: false,
};
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
  /** The terminal emulator for shell tabs; tests pass a fake. */
  createTerminal?: CreateTerminal;
  /** Opens an install or update link in the user's browser. */
  openLink?: OpenLink;
}

type Overlay = "palette" | "help" | "newSession" | "newWorktree" | "newTask" | "inbox" | null;

/** A session the user just asked for, by where it will run; it opens once the core confirms it. */
type PendingStart = { worktreePath: string } | { repositoryId: string; branch: string };

export function App({
  core,
  pickFolder,
  actions,
  createTerminal = xtermTerminal,
  openLink = (url) => window.open(url, "_blank"),
}: AppProps) {
  const registry = useMemo(() => actions ?? createActionRegistry(), [actions]);
  const [overlay, setOverlay] = useState<Overlay>(null);
  const [state, dispatch] = useReducer(appReducer, initialState);
  const [prefs, setPrefs] = useState(loadPrefs);
  const [sidebarOpen, setSidebarOpen] = useState(false); // overlay below 860 px
  const [selectedSession, setSelectedSession] = useState<string | null>(null);
  const [pendingStart, setPendingStart] = useState<PendingStart | null>(null);
  const [newSessionPreset, setNewSessionPreset] = useState<NewSessionPreset>({});
  const openNewSession = useCallback((preset: NewSessionPreset) => {
    setNewSessionPreset(preset);
    setOverlay("newSession");
  }, []);

  const updatePrefs = useCallback((patch: Partial<Prefs>) => {
    setPrefs((p) => {
      const next = { ...p, ...patch };
      savePrefs(next);
      return next;
    });
  }, []);

  const [streams, setStreams] = useState<ShellStreams | null>(null);
  useEffect(() => {
    // Shell output is buffered from the first event so a tab drawn later misses nothing.
    const shells = shellStreams(core);
    setStreams(shells);
    const unsubscribe = core.subscribe((event) => dispatch({ kind: "event", event }));
    core.send({ type: "attach" });
    return () => {
      unsubscribe();
      shells.dispose();
    };
  }, [core]);

  // The core watches git and pushes every change (ADR-0008); only the Forge is polled. On
  // returning to the app, poll it now; while hidden, the core polls it at its slowest.
  useEffect(() => {
    const onFocus = () => core.send({ type: "pollForgeNow" });
    const onVisibility = () => core.send({ type: "windowVisibility", visible: document.visibilityState !== "hidden" });
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [core]);

  // Bumped to make the session view scroll to its pending request (⌘J, inbox, toast).
  const [reveal, setReveal] = useState<number | undefined>(undefined);

  const sessions = Object.values(state.sessions);
  const openSession = useCallback(
    (id: string, revealRequest = false) => {
      setSelectedSession(id);
      setReveal(revealRequest ? (r) => (r ?? 0) + 1 : undefined);
      updatePrefs({ view: "session" });
    },
    [updatePrefs],
  );
  const answer = (sessionId: string, requestId: string, verdict: Verdict) =>
    core.send({ type: "answerRequest", sessionId, requestId, verdict });
  const waiting = waitingSessions(state);
  useEffect(() => {
    const started =
      pendingStart &&
      sessions.find((s) =>
        "worktreePath" in pendingStart
          ? s.worktreePath === pendingStart.worktreePath
          : s.repositoryId === pendingStart.repositoryId && s.branch === pendingStart.branch,
      );
    if (started) {
      setPendingStart(null);
      openSession(started.id);
    }
  }, [pendingStart, sessions, openSession]);
  const startSession = (command: Command) => {
    if (command.type === "startSession") setPendingStart({ worktreePath: command.worktreePath });
    if (command.type === "startSessionOnNewWorktree")
      setPendingStart({ repositoryId: command.repositoryId, branch: command.branch });
    if (command.type === "startTaskSession") {
      const repositoryId = taskById(state, command.taskId)?.repositoryId;
      const { target } = command;
      if (target.type === "worktree") setPendingStart({ worktreePath: target.worktreePath });
      if (target.type === "newWorktree" && repositoryId) setPendingStart({ repositoryId, branch: target.branch });
    }
    core.send(command);
  };

  // "Pick one" in Context: Tasks opens to link a task; once the link changes, back to the session.
  const [linking, setLinking] = useState<(Linking & { from: string | undefined }) | null>(null);
  const pickTask = useCallback(
    (sessionId: string, relink: boolean) => {
      setLinking({ sessionId, relink, from: state.taskLinks[sessionId] });
      updatePrefs({ view: "tasks" });
    },
    [state.taskLinks, updatePrefs],
  );
  useEffect(() => {
    if (linking && state.taskLinks[linking.sessionId] !== linking.from) {
      setLinking(null);
      openSession(linking.sessionId);
    }
  }, [linking, state.taskLinks, openSession]);
  useEffect(() => {
    if (prefs.view !== "tasks") setLinking(null);
  }, [prefs.view]);
  const shownSession = (selectedSession && state.sessions[selectedSession]) || sessions[sessions.length - 1];

  useEffect(() => {
    if (prefs.theme) document.documentElement.dataset.theme = prefs.theme;
    else delete document.documentElement.dataset.theme;
  }, [prefs.theme]);

  const dismiss = useCallback((id: number) => dispatch({ kind: "dismiss", id }), []);

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

  // Shells open at a nominal size; the terminal fits itself to the dock and resizes the PTY.
  const openShell = useCallback(
    (worktreePath: string) => {
      core.send({ type: "openShell", worktreePath, cols: 80, rows: 24 });
      updatePrefs({ dockCollapsed: false });
    },
    [core, updatePrefs],
  );

  const toggleDock = useCallback(
    () => updatePrefs({ dockCollapsed: !prefs.dockCollapsed }),
    [prefs.dockCollapsed, updatePrefs],
  );

  useHandler(registry, "palette.toggle", () => setOverlay((o) => (o === "palette" ? null : "palette")));
  useHandler(registry, "help.open", () => setOverlay("help"));
  useHandler(registry, "session.new", () => openNewSession({}));
  useHandler(registry, "sidebar.toggle", toggleSidebar);
  useHandler(registry, "dock.toggle", toggleDock);
  const toggleReview = useCallback(
    () => updatePrefs({ reviewHidden: !prefs.reviewHidden }),
    [prefs.reviewHidden, updatePrefs],
  );
  useHandler(registry, "review.togglePane", toggleReview);
  // The session workbench: review pane expanded to full width, or (below 860 px) shown in
  // place of the agent pane; text Ask agent drafted for the composer.
  const [bench, setBench] = useState<{ expanded: boolean; showChanges: boolean; draft: Draft }>({
    expanded: false,
    showChanges: false,
    draft: { text: "", seq: 0 },
  });
  // Review changes (the Finished and Paused boxes): the review pane, shown, on Changes.
  const [reviewSeq, setReviewSeq] = useState(0);
  const reviewChanges = useCallback(() => {
    if (prefs.reviewHidden) updatePrefs({ reviewHidden: false });
    setBench((b) => ({ ...b, showChanges: true }));
    setReviewSeq((n) => n + 1);
  }, [prefs.reviewHidden, updatePrefs]);
  useHandler(registry, "view.dashboard", () => updatePrefs({ view: "dashboard" }));
  useHandler(registry, "view.session", () => updatePrefs({ view: "session" }));
  useHandler(registry, "view.worktrees", () => updatePrefs({ view: "worktrees" }));
  useHandler(registry, "view.tasks", () => updatePrefs({ view: "tasks" }));
  // ⌘J: the next session needing the user after the one on screen, scrolled to its request.
  useHandler(registry, "needsYou.next", () => {
    if (!waiting.length) return dispatch({ kind: "result", text: "Nothing is waiting on you." });
    const current = prefs.view === "session" ? waiting.findIndex((s) => s.id === shownSession?.id) : -1;
    setOverlay(null);
    openSession(waiting[(current + 1) % waiting.length].id, true);
  });
  // Esc is layered: close an overlay, else cancel an inline confirm, else interrupt the agent.
  useHandler(registry, "escape", () => {
    if (overlay) return setOverlay(null);
    if (sidebarOpen) return setSidebarOpen(false);
    return registry.run("confirm.cancel") || registry.run("session.interrupt");
  });

  usePaletteSourceIn(registry, () => {
    const items: PaletteItem[] = [];
    for (const s of sessions)
      items.push({
        id: s.id,
        group: "Sessions",
        title: s.branch ?? s.worktreePath,
        detail: STATUS_LABEL[s.status],
        run: () => openSession(s.id),
      });
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
        items.push({
          id: `shell:${w.path}`,
          group: "Actions",
          title: `Shell in ${r.name}/${branch}`,
          keywords: [branch],
          run: () => openShell(w.path),
        });
      }
    for (const task of Object.values(state.tasks).flat())
      items.push({
        id: `task:${task.id}`,
        group: "Tasks",
        title: task.title,
        keywords: [task.key],
        detail: task.key,
        run: () => updatePrefs({ view: "tasks" }),
      });
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
    if (state.repositories.length > 0) {
      items.push({ id: "newWorktree", group: "Actions", title: "New worktree", run: () => setOverlay("newWorktree") });
      items.push({ id: "newTask", group: "Actions", title: "New local task", run: () => setOverlay("newTask") });
    }
    return items;
  });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const binding = BINDINGS.find((b) => matchesChord(e, b.chord));
      if (!binding || binding.scope === "composer" || binding.scope === "terminal") return;
      // Inside a shell tab every key belongs to the shell, Esc included; ⌘ chords still work.
      if (binding.action === "escape" && (e.target as HTMLElement | null)?.closest?.("[data-terminal]")) return;
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
            title={`Needs you (${keysLabel("needsYou.next")})`}
            onClick={() => setOverlay((o) => (o === "inbox" ? null : "inbox"))}
          >
            <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
              <path d="M2 9l2-6h8l2 6v4H2z" />
              <path d="M2 9h3.5l1 2h3l1-2H14" />
            </svg>
            <span className="lbl-sr">Needs you</span>
            {waiting.length > 0 && <span className="count">{waiting.length}</span>}
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
          <button className="btn primary" title={`New session (${keysLabel("session.new")})`} onClick={() => registry.run("session.new")}>
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
            {waiting.length > 0 && (
              <button className="needs" title="Open the queue of agents waiting on you" onClick={() => setOverlay("inbox")}>
                {waiting.length} need{waiting.length === 1 ? "s" : ""} you
              </button>
            )}
            <button className="btn ghost sm" title="Open a repository" onClick={addRepository}>
              + Repo
            </button>
          </div>
          <div className="sidebar-list">
            {state.repositories.length === 0 ? (
              <p className="empty">No repositories yet. Add one with + Repo.</p>
            ) : (
              state.repositories.map((r) => (
                <RepositoryGroup key={r.id} repository={r} state={state} onOpenSession={openSession} />
              ))
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
          {prefs.view === "session" && shownSession ? (
            <div
              className={[
                "workbench",
                prefs.reviewHidden ? "changes-hidden" : "",
                bench.expanded && !prefs.reviewHidden ? "focus-changes" : "",
                bench.showChanges ? "show-changes" : "",
              ].join(" ")}
            >
              <SessionView
                key={shownSession.id}
                session={shownSession}
                send={core.send}
                pending={state.requests[shownSession.id] ?? NONE}
                policy={state.policies[shownSession.repositoryId]}
                revealRequest={reveal}
                branchState={
                  <>
                    <BehindBadge
                      facts={branchOf(state, shownSession.worktreePath).facts}
                      base={branchOf(state, shownSession.worktreePath).repository?.baseBranch}
                    />
                    <PrBadge state={state} worktreePath={shownSession.worktreePath} />
                  </>
                }
                pullRequestActions={
                  <PullRequestActions
                    state={state}
                    worktreePath={shownSession.worktreePath}
                    send={core.send}
                    openLink={openLink}
                    primary={shownSession.status === "done"}
                  />
                }
                draft={bench.draft}
                reviewHidden={prefs.reviewHidden}
                onToggleReview={toggleReview}
                showChanges={bench.showChanges}
                onToggleChanges={() => setBench((b) => ({ ...b, showChanges: !b.showChanges }))}
                onReviewChanges={reviewChanges}
                onOpenTerminal={() => openShell(shownSession.worktreePath)}
              />
              {!prefs.reviewHidden && (
                <BranchPane
                  key={`pane-${shownSession.id}-${reviewSeq}`}
                  state={state}
                  session={shownSession}
                  send={core.send}
                  openLink={openLink}
                  expanded={bench.expanded}
                  onToggleExpand={() => setBench((b) => ({ ...b, expanded: !b.expanded }))}
                  onAsk={(text) =>
                    setBench((b) => ({ expanded: false, showChanges: false, draft: { text, seq: b.draft.seq + 1 } }))
                  }
                  onPickTask={pickTask}
                />
              )}
            </div>
          ) : prefs.view === "tasks" ? (
            <TasksView
              state={state}
              send={core.send}
              onAssign={(task) => openNewSession({ task, houseRules: state.houseRules[task.repositoryId] })}
              onOpenSession={(id) => openSession(id)}
              onNewTask={() => setOverlay("newTask")}
              linking={linking}
              onCancelLinking={() => setLinking(null)}
            />
          ) : prefs.view === "worktrees" ? (
            <WorktreesView
              state={state}
              send={core.send}
              onOpenSession={(id) => openSession(id)}
              onNewSession={openNewSession}
              onNewWorktree={() => setOverlay("newWorktree")}
              onOpenShell={openShell}
              openLink={openLink}
            />
          ) : prefs.view === "dashboard" ? (
            <Dashboard
              state={state}
              onOpenSession={(id) => openSession(id, true)}
              onOpenInbox={() => setOverlay("inbox")}
              onNewSession={openNewSession}
              setup={
                <SetupPanel
                  agents={state.agents}
                  forge={state.forge}
                  onCheck={() => {
                    core.send({ type: "checkAgents" });
                    core.send({ type: "checkForge" });
                  }}
                  openLink={openLink}
                />
              }
              spend={<SpendToday sessions={sessions} variant="kpi" />}
            />
          ) : (
            <div className="page">
              <div className="pagehead">
                <h1>{VIEWS.find(([v]) => v === prefs.view)?.[1]}</h1>
              </div>
              <p className="empty">
                {prefs.view === "session" ? "No session yet. Start one with New session." : "Nothing here yet."}
              </p>
            </div>
          )}
        </main>

        <Dock
          state={state}
          send={core.send}
          streams={streams}
          createTerminal={createTerminal}
          collapsed={prefs.dockCollapsed}
          onToggle={toggleDock}
          currentWorktree={prefs.view === "session" ? (shownSession?.worktreePath ?? null) : null}
          rawEvents={prefs.rawEvents}
          onRawEvents={(rawEvents) => updatePrefs({ rawEvents })}
          onOpenShell={openShell}
        />

        <footer className="statusbar">
          <span className="i">
            <i className="dot running" />
            {sessions.filter((s) => s.status === "running").length} running
          </span>
          <span className="i">
            <i className="dot waiting" />
            {sessions.filter((s) => s.status === "waiting").length} Needs you
          </span>
          <div className="r">
            <span className="i hide-sm">{state.repositories.length} repos</span>
            <SpendToday sessions={sessions} variant="status" />
          </div>
        </footer>

        <div className="toasts" role="status" aria-live="polite">
          {state.toasts.map((t) => (
            <ToastView
              key={t.id}
              toast={t}
              dismiss={dismiss}
              runOnce={(r) => {
                answer(r.sessionId, r.requestId, { type: "allowOnce" });
                dispatch({ kind: "result", text: `Ran once in ${state.sessions[r.sessionId]?.branch ?? "the session"}.` });
              }}
              open={(r) => openSession(r.sessionId, true)}
            />
          ))}
        </div>
        {overlay === "palette" && <Palette onClose={closeOverlay} />}
        {overlay === "help" && <ShortcutsHelp onClose={closeOverlay} />}
        {overlay === "newSession" && (
          <NewSession
            state={state}
            preset={newSessionPreset}
            onStart={startSession}
            send={core.send}
            onClose={closeOverlay}
            openLink={openLink}
          />
        )}
        {overlay === "newWorktree" && <NewWorktree state={state} send={core.send} onClose={closeOverlay} />}
        {overlay === "newTask" && <NewTask state={state} send={core.send} onClose={closeOverlay} />}
        {overlay === "inbox" && (
          <Inbox
            state={state}
            answer={answer}
            onClose={closeOverlay}
            onOpenSession={(id) => {
              closeOverlay();
              openSession(id, true);
            }}
          />
        )}
      </div>
    </ActionsContext.Provider>
  );
}

const NONE: PendingRequest[] = [];

/** One toast with its own timer. A request toast offers Run once (or Answer, for a question). */
function ToastView(props: {
  toast: Toast;
  dismiss: (id: number) => void;
  runOnce: (request: PendingRequest) => void;
  open: (request: PendingRequest) => void;
}) {
  const { toast, dismiss, runOnce, open } = props;
  const { id, ms, request } = toast;
  useEffect(() => {
    const timer = setTimeout(() => dismiss(id), ms);
    return () => clearTimeout(timer);
  }, [id, ms, dismiss]);
  return (
    <div className={`toast ${toast.tone}`}>
      <i />
      <span className="tx">
        {toast.text}
        {toast.detail && <small>{toast.detail}</small>}
      </span>
      {request &&
        (request.request.type === "question" ? (
          <button className="btn sm" onClick={() => (dismiss(id), open(request))}>
            Answer
          </button>
        ) : (
          <button className="btn ok sm" onClick={() => runOnce(request)}>
            Run once
          </button>
        ))}
      <button className="x" aria-label="Dismiss" onClick={() => dismiss(id)}>
        ×
      </button>
    </div>
  );
}

function RepositoryGroup(props: { repository: RepositoryView; state: State; onOpenSession: (id: string) => void }) {
  const { repository, state, onOpenSession } = props;
  const [collapsed, setCollapsed] = useState(false);
  const needsYou = repository.worktrees.filter((w) => sessionOn(state, w.path)?.status === "waiting").length;
  return (
    <div className={`repo ${collapsed ? "collapsed" : ""}`} role="group" aria-label={repository.name}>
      <button className="repo-row" title={repository.path} onClick={() => setCollapsed((c) => !c)}>
        <svg className="chev" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5">
          <path d="M3 4.5l3 3 3-3" />
        </svg>
        <span className="name">{repository.name}</span>
        {needsYou > 0 && <span className="n needs-n">{needsYou} Needs you</span>}
        <span className="n">base {repository.baseBranch}</span>
      </button>
      <ul className="worktrees">
        {repository.worktrees.map((w) => {
          const occupant = sessionOn(state, w.path);
          const branch = (
            <span className="branch">
              {w.branch ?? "(detached)"}
              {w.primary && <span className="tag">primary</span>}
            </span>
          );
          return (
            <li key={w.path} className="worktree" title={w.path}>
              {occupant ? (
                <button className="wt-open" onClick={() => onOpenSession(occupant.id)}>
                  <span className={`dot st-${occupant.status}`} />
                  {branch}
                  <span className="meta">
                    {AGENT_SHORT[occupant.agent]} · <Status status={occupant.status} variant="dot" />
                    <BehindBadge facts={state.worktreeFacts[w.path]} />
                    <PrBadge state={state} worktreePath={w.path} />
                  </span>
                </button>
              ) : (
                <>
                  <span className="dot" />
                  {branch}
                  <span className="meta">
                    free
                    <BehindBadge facts={state.worktreeFacts[w.path]} />
                    <PrBadge state={state} worktreePath={w.path} />
                  </span>
                </>
              )}
            </li>
          );
        })}
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
