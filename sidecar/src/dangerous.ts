// The dangerous command classes (network, delete, migration, push), kept in step with the
// core's copy (crates/switchyard-core/src/policy/dangerous.rs); both are checked against
// test/dangerous-commands.json. Below Full auto the driver routes such commands to the
// host even when the CLI's mode (acceptEdits approves `rm`) or an allow rule would approve
// them; the core then lets only a grant that names them explicitly answer.

export type DangerClass = "network" | "delete" | "migration" | "push";

const NETWORK = ["curl", "wget", "ssh", "scp", "sftp", "rsync", "nc", "ncat", "telnet", "ftp"];
const DELETE = ["rm", "rmdir", "unlink", "shred", "-delete"];
/** Words that delete after `git`. */
const GIT_DELETE = ["clean", "rm"];
const ORDER: DangerClass[] = ["network", "delete", "migration", "push"];

/** The dangerous classes `command` is in, in a fixed order (empty: not dangerous). */
export function classes(command: string): DangerClass[] {
  const words = command
    .split(/[\s;|&`()<>$]+/)
    .filter((w) => w)
    .map((w) => {
      const bare = w.replace(/^["']+|["']+$/g, "");
      return bare.slice(bare.lastIndexOf("/") + 1).toLowerCase();
    });
  const found = new Set<DangerClass>();
  words.forEach((w, i) => {
    if (NETWORK.includes(w)) found.add("network");
    if (DELETE.includes(w) || (i > 0 && words[i - 1] === "git" && GIT_DELETE.includes(w))) found.add("delete");
    if (w.includes("migrat")) found.add("migration");
    if (w === "push") found.add("push");
  });
  return ORDER.filter((c) => found.has(c));
}
