import {
  Check,
  CornerDownRight,
  ListPlus,
  Loader2,
  RefreshCw,
  Settings,
  Sparkles,
} from "lucide-react";
import { type ReactNode, useState } from "react";
import { SelectionToTask } from "@/components/tasks/SelectionToTask";
import { Button } from "@/components/ui/button";
import { CoreError, describeError } from "@/lib/native";
import { isTauri } from "@/lib/platform";
import { useAddTasks, useGenerateNotes, useMeeting, useMeetingTasks } from "@/lib/queries";
import { addedKeys, taskKey } from "@/lib/tasks";
import type { MeetingDetail, MeetingNotes, NewTask, TaskSourceKind } from "@/lib/types";
import { cn } from "@/lib/utils";
import { useUi } from "@/stores/ui";

export function NotesPanel() {
  const selectedId = useUi((s) => s.selectedMeetingId);
  const { data: meeting } = useMeeting(selectedId);

  return (
    <aside className="flex w-[22rem] shrink-0 flex-col border-l bg-panel">
      <div className="flex h-14 shrink-0 items-center gap-2 border-b px-5">
        <Sparkles className="size-5 text-accent" />
        <h2 className="font-medium">AI Notes</h2>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-5">
        <NotesBody meeting={meeting} />
      </div>
    </aside>
  );
}

const OUTPUT_LANGUAGES = [
  { code: "om", label: "Afaan Oromo" },
  { code: "en", label: "English" },
] as const;

function NotesBody({ meeting }: { meeting: MeetingDetail | undefined }) {
  const generate = useGenerateNotes();
  const [language, setLanguage] = useState<string | null>(null);

  if (!meeting)
    return <Hint>Select a meeting to see its summary, decisions and action items.</Hint>;
  if (meeting.status === "recording")
    return <Hint>Notes are generated from the transcript after the meeting.</Hint>;
  if (meeting.segments.length === 0)
    return (
      <div className="space-y-3">
        <Hint>
          {meeting.status === "processing"
            ? "Waiting for the transcript…"
            : "Notes are made from the transcript once there is one."}
        </Hint>
        {meeting.status === "processing" &&
          [80, 95, 70].map((w) => (
            <div key={w} className="h-3 animate-pulse rounded bg-card" style={{ width: `${w}%` }} />
          ))}
      </div>
    );

  const busy = generate.isPending && generate.variables?.id === meeting.id;
  // Default: the language the notes were last written in, else the meeting's.
  const output = language ?? meeting.notes?.language ?? meeting.language;
  const run = () => generate.mutate({ id: meeting.id, outputLanguage: output });
  const error = generate.isError && generate.variables?.id === meeting.id ? generate.error : null;

  const controls = (
    <div className="space-y-2">
      {isTauri() && (
        <label className="flex items-center justify-between gap-2 text-xs text-muted">
          Write notes in
          <select
            value={output}
            onChange={(e) => setLanguage(e.target.value)}
            disabled={busy}
            className="h-8 rounded-md border bg-bg px-2 text-xs text-fg outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
          >
            {OUTPUT_LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>
                {l.label}
              </option>
            ))}
          </select>
        </label>
      )}
      <Button
        className="w-full"
        variant={meeting.notes ? "outline" : "default"}
        size={meeting.notes ? "sm" : "default"}
        disabled={busy}
        onClick={run}
      >
        {busy ? <Loader2 className="animate-spin" /> : meeting.notes ? <RefreshCw /> : <Sparkles />}
        {busy ? "Generating…" : meeting.notes ? "Regenerate" : "Generate notes"}
      </Button>
      {error && <NotesError error={error} />}
    </div>
  );

  if (!meeting.notes)
    return (
      <div className="space-y-4">
        <Hint>
          Create a summary with key points, decisions and action items from this transcript. The
          transcript text is sent to Addis AI.
        </Hint>
        {controls}
      </div>
    );

  return (
    <div className="space-y-6">
      <Notes notes={meeting.notes} meeting={meeting} />
      {controls}
    </div>
  );
}

function NotesError({ error }: { error: Error }) {
  const code = error instanceof CoreError ? error.code : null;
  const openSettings = () => useUi.getState().setSettingsOpen(true);
  const text = (() => {
    switch (code) {
      case "provider_auth":
        return "Add a working Addis AI API key in Settings to generate notes.";
      case "provider_credits":
        return "Your Addis AI balance is too low. Top up, then try again.";
      case "provider_unavailable":
        return "Couldn't reach Addis AI (offline or busy). Try again in a moment.";
      case "provider_failed":
        return `The notes model couldn't produce notes: ${error.message}`;
      default:
        return describeError(error);
    }
  })();
  return (
    <div className="space-y-2 rounded-lg bg-warn/10 px-3 py-2 text-xs text-warn">
      <p>{text}</p>
      {(code === "provider_auth" || code === "provider_credits") && (
        <Button size="sm" variant="outline" onClick={openSettings}>
          <Settings />
          Open Settings
        </Button>
      )}
    </div>
  );
}

function Notes({ notes, meeting }: { notes: MeetingNotes; meeting: MeetingDetail }) {
  const seek = useUi((s) => s.seek);
  const jump = (segmentIds: string[]) => {
    const seg = meeting.segments.find((s) => s.id === segmentIds[0]);
    if (seg) seek(seg.startMs);
  };

  // Tasks from these notes (FR-AI-08). Tasks are copies, so they stay
  // marked across regenerations that produce the same item.
  const { data: tasks = [] } = useMeetingTasks(meeting.id);
  const added = addedKeys(tasks);
  const add = useAddTasks();
  const item = (
    sourceKind: TaskSourceKind,
    text: string,
    segmentIds: string[],
    owner?: string,
    due?: string,
  ): NewTask => ({
    meetingId: meeting.id,
    text,
    owner,
    due,
    sourceKind,
    sourceVersion: notes.version ?? null,
    segmentIds,
  });
  const isAdded = (kind: TaskSourceKind, text: string) => added.has(taskKey(kind, text));
  const pendingKeys = new Set(
    add.isPending ? (add.variables ?? []).map((v) => taskKey(v.sourceKind, v.text)) : [],
  );
  const addButton = (n: NewTask, subtle: boolean) => (
    <AddToTasks
      added={isAdded(n.sourceKind, n.text)}
      pending={pendingKeys.has(taskKey(n.sourceKind, n.text))}
      subtle={subtle}
      onAdd={() => add.mutate([n])}
    />
  );
  const missing = notes.actionItems
    .filter((a) => !isAdded("action_item", a.text))
    .map((a) => item("action_item", a.text, a.segmentIds, a.owner, a.due));

  return (
    // Select any text (summary included) to add it as a task.
    <SelectionToTask
      meetingId={meeting.id}
      sourceVersion={notes.version ?? null}
      kind="summary"
      className="space-y-6"
    >
      <Section
        title="Summary"
        action={<span className="text-xs text-muted">Select text to add a task</span>}
      >
        <p data-task-kind="summary" className="text-sm leading-relaxed text-fg/90">
          {notes.summary}
        </p>
      </Section>

      <Section title="Key points">
        <ul className="space-y-1.5 text-sm text-fg/90">
          {notes.keyPoints.map((p) => (
            <li key={p} data-task-kind="key_point" className="group flex items-start gap-2">
              <span className="mt-2 size-1.5 shrink-0 rounded-full bg-accent" />
              <span className="min-w-0 flex-1">{p}</span>
              {addButton(item("key_point", p, []), true)}
            </li>
          ))}
        </ul>
      </Section>

      <Section title="Decisions">
        <ul className="space-y-2">
          {notes.decisions.map((d) => (
            <li
              key={d.text}
              data-task-kind="decision"
              data-segment-ids={d.segmentIds.join(" ")}
              className="group flex items-start gap-1"
            >
              <SourceLink onClick={() => jump(d.segmentIds)}>{d.text}</SourceLink>
              {addButton(item("decision", d.text, d.segmentIds), true)}
            </li>
          ))}
        </ul>
      </Section>

      <Section
        title="Action items"
        action={
          notes.actionItems.length > 0 && (
            <button
              type="button"
              onClick={() => add.mutate(missing)}
              disabled={missing.length === 0 || add.isPending}
              className="rounded-md px-1.5 py-0.5 text-xs font-medium text-accent transition-colors hover:bg-accent-soft disabled:pointer-events-none disabled:text-muted"
            >
              {missing.length === 0 ? "All in tasks" : "Add all to tasks"}
            </button>
          )
        }
      >
        <ul className="space-y-2">
          {notes.actionItems.map((a) => (
            <li
              key={a.text}
              data-task-kind="action_item"
              data-segment-ids={a.segmentIds.join(" ")}
              className="group flex items-start gap-1"
            >
              <SourceLink onClick={() => jump(a.segmentIds)}>
                <span>{a.text}</span>
                {(a.owner || a.due) && (
                  <span className="mt-0.5 block text-xs text-muted">
                    {[a.owner, a.due].filter(Boolean).join(" · ")}
                  </span>
                )}
              </SourceLink>
              {addButton(item("action_item", a.text, a.segmentIds, a.owner, a.due), false)}
            </li>
          ))}
        </ul>
        {add.isError && (
          <p className="text-xs text-warn">Couldn't add to tasks: {describeError(add.error)}</p>
        )}
      </Section>

      {notes.keywords.length > 0 && (
        <Section title="Keywords">
          <div className="flex flex-wrap gap-2 select-none">
            {notes.keywords.map((k) => (
              <span key={k} className="rounded-md border bg-card px-2 py-1 text-xs text-fg/90">
                {k}
              </span>
            ))}
          </div>
        </Section>
      )}
    </SelectionToTask>
  );
}

function Section({
  title,
  action,
  children,
}: {
  title: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="space-y-2.5 border-b pb-5 last-of-type:border-0">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-sm font-semibold">{title}</h3>
        {action}
      </div>
      {children}
    </section>
  );
}

// Adds one notes item to tasks. Once added it shows a check; clicking that
// opens the Tasks view. `subtle` items (key points, decisions) show the
// button on hover only.
function AddToTasks({
  added,
  pending,
  subtle,
  onAdd,
}: {
  added: boolean;
  pending: boolean;
  subtle: boolean;
  onAdd: () => void;
}) {
  const label = added ? "In tasks: open Tasks" : "Add to tasks";
  return (
    <button
      type="button"
      onClick={added ? () => useUi.getState().setView("tasks") : onAdd}
      disabled={pending}
      aria-label={label}
      title={label}
      className={cn(
        "grid size-6 shrink-0 place-items-center rounded-md transition-all focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-accent/60 focus-visible:outline-none [&_svg]:size-3.5",
        added
          ? "text-accent hover:bg-accent-soft"
          : "text-muted hover:bg-card-hover hover:text-accent",
        subtle && !added && !pending && "opacity-0 group-hover:opacity-100",
      )}
    >
      {pending ? <Loader2 className="animate-spin" /> : added ? <Check /> : <ListPlus />}
    </button>
  );
}

function SourceLink({ children, onClick }: { children: ReactNode; onClick: () => void }) {
  return (
    <button
      type="button"
      // A drag that selected text (to add it as a task) isn't a jump.
      onClick={() => window.getSelection()?.isCollapsed !== false && onClick()}
      title="Jump to transcript"
      className="group/link flex min-w-0 flex-1 select-text gap-2 rounded-md p-1 -m-1 text-left text-sm text-fg/90 hover:bg-card-hover"
    >
      <CornerDownRight className="mt-0.5 size-3.5 shrink-0 text-muted group-hover/link:text-accent" />
      <span>{children}</span>
    </button>
  );
}

function Hint({ children }: { children: ReactNode }) {
  return <p className="text-sm leading-relaxed text-muted">{children}</p>;
}
