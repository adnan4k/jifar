import type { Task, TaskSourceKind } from "./types";

/**
 * How a notes item is recognised as already added: trimmed, whitespace
 * collapsed, case and Qubee apostrophes kept. Must match
 * `jifar_core::tasks::match_key`.
 */
export function matchKey(text: string): string {
  return text.trim().split(/\s+/).filter(Boolean).join(" ");
}

/** Key for "is this notes item in tasks?" lookups. */
export function taskKey(kind: TaskSourceKind, text: string): string {
  return `${kind}\u0000${matchKey(text)}`;
}

export function addedKeys(tasks: Task[]): Set<string> {
  return new Set(
    tasks.filter((t) => t.sourceKind !== "manual").map((t) => `${t.sourceKind}\u0000${t.matchKey}`),
  );
}
/** "Click again to delete" plus how many tasks go with it (FR-AI-08). */
export function deleteConfirmText(taskCount: number | undefined): string {
  if (!taskCount) return "Click again to delete";
  return `Click again to delete (and ${taskCount} ${taskCount === 1 ? "task" : "tasks"})`;
}
