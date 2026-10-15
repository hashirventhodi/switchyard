// Renderer state is derived only from core events.
import type { Event } from "./schema/Event";
import type { Repository } from "./schema/Repository";
import type { Worktree } from "./schema/Worktree";

export interface RepositoryView extends Repository {
  worktrees: Worktree[];
}

export interface Toast {
  id: number;
  tone: "bad" | "ok" | "info" | "warn";
  text: string;
}

export interface State {
  repositories: RepositoryView[];
  toasts: Toast[];
}

export const initialState: State = { repositories: [], toasts: [] };

let toastSeq = 0;

export function reduce(state: State, event: Event): State {
  switch (event.type) {
    case "attached":
      return { ...state, repositories: [] };
    case "repositoryAdded": {
      const others = state.repositories.filter((r) => r.id !== event.repository.id);
      return { ...state, repositories: [...others, { ...event.repository, worktrees: [] }] };
    }
    case "worktreeAdded":
      return {
        ...state,
        repositories: state.repositories.map((r) =>
          r.id === event.repositoryId
            ? { ...r, worktrees: [...r.worktrees.filter((w) => w.path !== event.worktree.path), event.worktree] }
            : r,
        ),
      };
    case "commandRefused":
      return { ...state, toasts: [...state.toasts, { id: ++toastSeq, tone: "bad", text: event.reason }] };
  }
}

export function dismissToast(state: State, id: number): State {
  return { ...state, toasts: state.toasts.filter((t) => t.id !== id) };
}
