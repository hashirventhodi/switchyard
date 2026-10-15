// Branch names suggested from a prompt: `<kind>/<three-words>`.
const STOP = new Set(
  "the a an to and of for in on with from add fix make update per via get post put new".split(" "),
);

export type BranchKind = "feat" | "fix" | "chore";

/** `feat/cursor-pagination-orders` from "Add cursor pagination to orders"; "" for no words. */
export function slugBranch(text: string, kind?: BranchKind): string {
  const words = text
    .toLowerCase()
    .replace(/[^a-z0-9\s-]/g, "")
    .trim()
    .split(/\s+/)
    .filter((w) => w && !STOP.has(w))
    .slice(0, 3)
    .join("-");
  const guessed: BranchKind = /fix|bug|flak|broken|fail/i.test(text)
    ? "fix"
    : /refactor|clean|migrat|replace|remove|upgrade|move/i.test(text)
      ? "chore"
      : "feat";
  return words ? `${kind ?? guessed}/${words}` : "";
}
