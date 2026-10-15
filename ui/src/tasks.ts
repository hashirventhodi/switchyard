// Task vocabulary shared by the Tasks view, New session and Context.
import { slugBranch } from "./branch";
import type { Task } from "./schema/Task";
import type { TaskColumn } from "./schema/TaskColumn";
import type { TaskSourceKind } from "./schema/TaskSourceKind";

export const COLUMNS: [TaskColumn, string][] = [
  ["todo", "To do"],
  ["inProgress", "In progress"],
  ["inReview", "In review"],
  ["done", "Done"],
];
export const COLUMN_LABEL = Object.fromEntries(COLUMNS) as Record<TaskColumn, string>;

export const SOURCE_LABEL: Record<TaskSourceKind, string> = { local: "local", github: "GitHub" };

/** `<kind>/<key>-<slug>`: `feat/l1-cursor-pagination-orders` for L-1 "Cursor pagination on orders". */
export function taskBranch(task: Task): string {
  const key = task.key.replace(/[^a-z0-9]/gi, "").toLowerCase();
  const slug = slugBranch(task.title, task.kind).replace(/^[^/]*\//, "");
  return `${task.kind}/${[key, slug].filter(Boolean).join("-")}`;
}

/** The initial prompt for a session on a task: the ticket, then the repository's house rules. */
export function taskPrompt(task: Task, houseRules: string | undefined): string {
  return [`Work on ${task.key}: ${task.title}.`, task.description.trim(), houseRules?.trim()].filter(Boolean).join("\n\n");
}
