import {
  Check,
  CornerDownRight,
  ListChecks,
  Loader2,
  Pencil,
  Plus,
  Settings,
  Trash2,
  X,
} from "lucide-react";
import { type FormEvent, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { describeError } from "@/lib/native";
import { useAddTasks, useDeleteTask, useTaskCounts, useTasks, useUpdateTask } from "@/lib/queries";
import type { Task, TaskFilter } from "@/lib/types";
import { cn, formatTimestamp } from "@/lib/utils";
import { useUi } from "@/stores/ui";

// Tasks across meetings (FR-AI-08, spec D12): a local checklist of
// follow-ups. Owner and due date are free text, never scheduled.
export function TasksPane() {
  const search = useUi((s) => s.search);
  const [filter, setFilter] = useState<TaskFilter>("open");
  const { data: tasks = [], isPending } = useTasks(filter, search);

  return (
    <main className="flex min-w-0 flex-1 flex-col">
      <header className="flex h-14 shrink-0 items-center justify-between gap-3 border-b px-6">
        <div className="flex min-w-0 items-center gap-2">
          <ListChecks className="size-5 shrink-0 text-accent" />
          <h1 className="truncate font-medium">Tasks</h1>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <FilterTabs value={filter} onChange={setFilter} />
          <Button
            variant="outline"
            size="sm"
            onClick={() => useUi.getState().setSettingsOpen(true)}
          >
            <Settings />
            Settings
          </Button>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex max-w-3xl flex-col gap-6 px-6 py-5">
          <AddTaskForm />
          {!isPending && tasks.length === 0 ? (
            <EmptyTasks filter={filter} searching={search.trim() !== ""} />
          ) : (
            groupByMeeting(tasks).map((g) => <TaskGroup key={g.key} group={g} />)
          )}
        </div>
      </div>
    </main>
  );
}

function FilterTabs({ value, onChange }: { value: TaskFilter; onChange: (f: TaskFilter) => void }) {
  const { data: counts } = useTaskCounts();
  const tabs: { id: TaskFilter; label: string; count?: number }[] = [
    { id: "open", label: "Open", count: counts?.open },
    { id: "done", label: "Done", count: counts?.done },
    { id: "all", label: "All" },
  ];
  return (
    <div role="tablist" aria-label="Show" className="flex rounded-lg border bg-bg p-0.5">
      {tabs.map((t) => (
        <button
          key={t.id}
          type="button"
          role="tab"
          aria-selected={value === t.id}
          onClick={() => onChange(t.id)}
          className={cn(
            "rounded-md px-2.5 py-1 text-xs font-medium transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent/60",
            value === t.id ? "bg-card text-fg" : "text-muted hover:text-fg",
          )}
        >
          {t.label}
          {t.count !== undefined && (
            <span className="ml-1.5 text-muted tabular-nums">{t.count}</span>
          )}
        </button>
      ))}
    </div>
  );
}

interface Group {
  key: string;
  meetingId: string | null;
  title: string;
  tasks: Task[];
}

/** Keeps the list order (open first, newest first); groups by meeting. */
function groupByMeeting(tasks: Task[]): Group[] {
  const groups = new Map<string, Group>();
  for (const t of tasks) {
    const key = t.meetingId ?? "";
    let g = groups.get(key);
    if (!g) {
      g = {
        key,
        meetingId: t.meetingId,
        title: t.meetingId ? (t.meetingTitle ?? "Meeting") : "Added by hand",
        tasks: [],
      };
      groups.set(key, g);
    }
    g.tasks.push(t);
  }
  return [...groups.values()];
}

function TaskGroup({ group }: { group: Group }) {
  const { meetingId } = group;
  return (
    <section className="flex flex-col gap-1">
      {meetingId ? (
        <button
          type="button"
          onClick={() => useUi.getState().selectMeeting(meetingId)}
          title="Open meeting"
          className="self-start truncate rounded-md px-1 text-xs font-medium text-muted hover:text-accent"
        >
          {group.title}
        </button>
      ) : (
        <h2 className="px-1 text-xs font-medium text-muted">{group.title}</h2>
      )}
      <ul className="divide-y rounded-lg border bg-panel">
        {group.tasks.map((t) => (
          <TaskItem key={t.id} task={t} />
        ))}
      </ul>
    </section>
  );
}

/** Shown under a task; action items and typed tasks need no label. */
const KIND_LABEL: Record<Task["sourceKind"], string | null> = {
  action_item: null,
  decision: "Decision",
  key_point: "Key point",
  summary: "From summary",
  transcript: "From transcript",
  manual: null,
};

function TaskItem({ task }: { task: Task }) {
  const update = useUpdateTask();
  const [editing, setEditing] = useState(false);
  const toggle = () => update.mutate({ id: task.id, patch: { done: !task.done } });
  const kind = KIND_LABEL[task.sourceKind];
  const { meetingId, sourceStartMs } = task;

  if (editing) return <TaskEditor task={task} onClose={() => setEditing(false)} />;

  return (
    <li className="group flex items-start gap-3 px-3 py-2.5">
      <span className="relative mt-0.5 grid size-4 shrink-0 place-items-center">
        <input
          id={`task-done-${task.id}`}
          type="checkbox"
          checked={task.done}
          onChange={toggle}
          disabled={update.isPending}
          aria-label={task.done ? "Mark as open" : "Mark as done"}
          className="peer size-4 cursor-pointer appearance-none rounded border border-muted transition-colors outline-none checked:border-accent checked:bg-accent hover:border-accent focus-visible:ring-2 focus-visible:ring-accent/60"
        />
        <Check
          strokeWidth={3}
          className="pointer-events-none absolute hidden size-3 text-white peer-checked:block"
        />
      </span>

      <div className="min-w-0 flex-1">
        <p className={cn("text-sm break-words", task.done && "text-muted line-through")}>
          {task.text}
        </p>
        {(task.owner || task.due || kind || (meetingId && sourceStartMs !== null)) && (
          <div className="mt-0.5 flex flex-wrap items-center gap-x-1.5 text-xs text-muted">
            {[kind, task.owner, task.due].filter(Boolean).join(" · ")}
            {(kind || task.owner || task.due) && meetingId && sourceStartMs !== null && (
              <span aria-hidden>·</span>
            )}
            {meetingId && sourceStartMs !== null && (
              <button
                type="button"
                onClick={() => useUi.getState().openAt(meetingId, sourceStartMs)}
                title="Jump to where it was said"
                className="inline-flex items-center gap-1 rounded hover:text-accent [&_svg]:size-3"
              >
                <CornerDownRight />
                <span className="tabular-nums">{formatTimestamp(sourceStartMs)}</span>
              </button>
            )}
          </div>
        )}
        {update.isError && <p className="mt-1 text-xs text-warn">{describeError(update.error)}</p>}
      </div>

      <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100">
        <Button
          variant="ghost"
          size="icon"
          className="size-7 [&_svg]:size-3.5"
          aria-label="Edit task"
          title="Edit"
          onClick={() => setEditing(true)}
        >
          <Pencil />
        </Button>
        <DeleteTaskButton id={task.id} />
      </div>
    </li>
  );
}

// Text, owner and due date. Enter saves, Escape cancels.
function TaskEditor({ task, onClose }: { task: Task; onClose: () => void }) {
  const update = useUpdateTask();
  const [text, setText] = useState(task.text);
  const [owner, setOwner] = useState(task.owner ?? "");
  const [due, setDue] = useState(task.due ?? "");

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!text.trim()) return;
    update.mutate({ id: task.id, patch: { text, owner, due } }, { onSuccess: onClose });
  };
  const input =
    "h-8 rounded-md border bg-bg px-2 text-sm outline-none placeholder:text-muted focus-visible:ring-2 focus-visible:ring-accent/60";
  return (
    <li className="px-3 py-2.5">
      <form
        onSubmit={submit}
        onKeyDown={(e) => e.key === "Escape" && onClose()}
        className="flex flex-col gap-2"
      >
        <input
          id={`task-text-${task.id}`}
          // biome-ignore lint/a11y/noAutofocus: the user just asked to edit this task
          autoFocus
          value={text}
          onChange={(e) => setText(e.target.value)}
          aria-label="Task"
          className={cn(input, "w-full")}
        />
        <div className="flex flex-wrap items-center gap-2">
          <input
            id={`task-owner-${task.id}`}
            value={owner}
            onChange={(e) => setOwner(e.target.value)}
            placeholder="Owner"
            aria-label="Owner"
            className={cn(input, "w-36")}
          />
          <input
            id={`task-due-${task.id}`}
            value={due}
            onChange={(e) => setDue(e.target.value)}
            placeholder="Due (e.g. Jimaata)"
            aria-label="Due"
            className={cn(input, "w-44")}
          />
          <div className="ml-auto flex items-center gap-1">
            <Button type="button" variant="ghost" size="sm" onClick={onClose}>
              <X />
              Cancel
            </Button>
            <Button type="submit" size="sm" disabled={!text.trim() || update.isPending}>
              {update.isPending ? <Loader2 className="animate-spin" /> : <Check />}
              Save
            </Button>
          </div>
        </div>
        {update.isError && <p className="text-xs text-warn">{describeError(update.error)}</p>}
      </form>
    </li>
  );
}

// Two-step delete, like meetings: the first click arms it for 4 s.
function DeleteTaskButton({ id }: { id: string }) {
  const del = useDeleteTask();
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 4_000);
    return () => clearTimeout(t);
  }, [armed]);
  const label = armed ? "Click again to delete" : "Delete task";
  return (
    <Button
      variant={armed ? "danger" : "ghost"}
      size="icon"
      className="size-7 [&_svg]:size-3.5"
      aria-label={label}
      title={del.isError ? describeError(del.error) : label}
      disabled={del.isPending}
      onClick={() => {
        if (!armed) return setArmed(true);
        setArmed(false);
        del.mutate(id);
      }}
    >
      {del.isPending ? <Loader2 className="animate-spin" /> : <Trash2 />}
    </Button>
  );
}

// A task typed by hand, not tied to a meeting.
function AddTaskForm() {
  const add = useAddTasks();
  const [text, setText] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!text.trim()) return;
    add.mutate([{ meetingId: null, text, sourceKind: "manual", segmentIds: [] }], {
      onSuccess: () => setText(""),
    });
  };
  return (
    <form onSubmit={submit} className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <input
          id="task-new"
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Add a task…"
          aria-label="New task"
          className="h-9 min-w-0 flex-1 rounded-lg border bg-bg px-3 text-sm outline-none placeholder:text-muted focus-visible:ring-2 focus-visible:ring-accent/60"
        />
        <Button type="submit" size="sm" className="h-9" disabled={!text.trim() || add.isPending}>
          {add.isPending ? <Loader2 className="animate-spin" /> : <Plus />}
          Add
        </Button>
      </div>
      {add.isError && <p className="text-xs text-warn">{describeError(add.error)}</p>}
    </form>
  );
}

function EmptyTasks({ filter, searching }: { filter: TaskFilter; searching: boolean }) {
  const text = searching
    ? "No tasks match your search."
    : filter === "done"
      ? "Nothing ticked off yet."
      : "No open tasks. Add action items from a meeting's notes, or type one above.";
  return (
    <div className="grid place-items-center py-16 text-center">
      <div className="max-w-sm space-y-3">
        <div className="mx-auto grid size-12 place-items-center rounded-full bg-accent-soft">
          <ListChecks className="size-5 text-accent" />
        </div>
        <p className="text-sm text-muted">{text}</p>
      </div>
    </div>
  );
}
