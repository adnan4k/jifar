import { AudioLines, ListChecks, Loader2, Search, Settings, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { StatusBadge } from "@/components/ui/status-badge";
import { useAppInfo } from "@/lib/native";
import { useDeleteMeeting, useMeetings, useMeetingTasks, useTaskCounts } from "@/lib/queries";
import { deleteConfirmText } from "@/lib/tasks";
import { cn, formatDate, formatDuration } from "@/lib/utils";
import { useUi } from "@/stores/ui";
import { DevicePicker, RecordControls, RecordingNotice, Waveform } from "./RecordControls";

export function Sidebar() {
  return (
    <aside className="flex w-72 shrink-0 flex-col border-r bg-panel">
      <div className="flex items-center gap-2.5 px-5 pt-5 pb-6">
        <AudioLines className="size-6 text-accent" />
        <span className="text-lg font-semibold tracking-tight">Jifar</span>
      </div>

      <div className="flex flex-col gap-4 px-4">
        <RecordControls />
        <DevicePicker />
        <Waveform />
        <RecordingNotice />
      </div>

      <MeetingList />

      <div className="border-t p-3">
        <button
          type="button"
          onClick={() => useUi.getState().setSettingsOpen(true)}
          className="flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm text-muted transition-colors hover:bg-card-hover hover:text-fg"
        >
          <Settings className="size-4" />
          Settings
        </button>
        <CoreStatus />
      </div>
    </aside>
  );
}

function MeetingList() {
  const { search, setSearch, selectedMeetingId, selectMeeting, view } = useUi();
  const { data: meetings = [] } = useMeetings(search);

  return (
    <div className="mt-4 flex min-h-0 flex-1 flex-col border-t pt-4">
      <label className="relative mx-4 mb-3 block">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted" />
        <input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder={view === "tasks" ? "Search tasks…" : "Search meetings…"}
          className="h-9 w-full rounded-lg border bg-bg pr-3 pl-8 text-sm outline-none placeholder:text-muted focus-visible:ring-2 focus-visible:ring-accent/60"
        />
      </label>

      <TasksLink />

      <p className="px-5 pb-1.5 text-xs font-medium tracking-wide text-muted uppercase">Meetings</p>
      <ul className="min-h-0 flex-1 space-y-0.5 overflow-y-auto px-2 pb-2">
        {meetings.length === 0 && (
          <li className="px-3 py-6 text-center text-sm text-muted">
            {search ? "No matches" : "No meetings yet"}
          </li>
        )}
        {meetings.map((m) => (
          <li key={m.id} className="group relative">
            <button
              type="button"
              onClick={() => selectMeeting(m.id)}
              className={cn(
                "w-full rounded-lg px-3 py-2 text-left transition-colors hover:bg-card-hover",
                view === "meetings" && selectedMeetingId === m.id && "bg-card",
              )}
            >
              <div className="truncate pr-6 text-sm">{m.title}</div>
              <div className="mt-0.5 flex items-center gap-2 text-xs text-muted">
                <span>{formatDate(m.startedAt)}</span>
                {m.status !== "recording" && <span>· {formatDuration(m.durationMs)}</span>}
                <StatusBadge status={m.status} className="ml-auto" />
              </div>
            </button>
            {m.status !== "recording" && (
              <RowDeleteButton
                meetingId={m.id}
                onDeleted={() => {
                  if (useUi.getState().selectedMeetingId === m.id) selectMeeting(null);
                }}
              />
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

// Opens the Tasks view (FR-AI-08), with the number of open tasks.
function TasksLink() {
  const active = useUi((s) => s.view === "tasks");
  const { data: counts } = useTaskCounts();
  return (
    <div className="px-2 pb-3">
      <button
        type="button"
        onClick={() => useUi.getState().setView("tasks")}
        aria-current={active ? "page" : undefined}
        className={cn(
          "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm transition-colors hover:bg-card-hover",
          active ? "bg-card text-fg" : "text-fg/90",
        )}
      >
        <ListChecks className="size-4 text-accent" />
        Tasks
        {counts && counts.open > 0 && (
          <span className="ml-auto rounded-full bg-accent-soft px-2 py-0.5 text-xs font-medium text-accent tabular-nums">
            {counts.open}
          </span>
        )}
      </button>
    </div>
  );
}

// Two-step delete, like the meeting header's: the first click arms it, the
// second (within 4 s) deletes the meeting, its audio and its tasks
// (FR-HIS-04, FR-AI-08).
function RowDeleteButton({ meetingId, onDeleted }: { meetingId: string; onDeleted: () => void }) {
  const del = useDeleteMeeting();
  const [armed, setArmed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { data: tasks } = useMeetingTasks(meetingId, armed);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 4_000);
    return () => clearTimeout(t);
  }, [armed]);
  const confirm = deleteConfirmText(tasks?.length);

  const click = () => {
    if (!armed) {
      setArmed(true);
      setError(null);
      return;
    }
    setArmed(false);
    del.mutate(meetingId, {
      onSuccess: onDeleted,
      onError: (e) => setError(e instanceof Error ? e.message : String(e)),
    });
  };
  const visible = armed || del.isPending || error;
  return (
    <button
      type="button"
      onClick={click}
      disabled={del.isPending}
      aria-label={armed ? confirm : "Delete meeting"}
      title={error ?? (armed ? confirm : "Delete this meeting and its audio")}
      className={cn(
        "absolute top-1.5 right-1.5 flex h-6 min-w-6 items-center justify-center gap-1 rounded-md px-1 text-[11px] font-medium transition-all focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-accent/60 focus-visible:outline-none [&_svg]:size-3.5",
        armed
          ? "bg-danger text-white hover:bg-danger/90"
          : error
            ? "text-warn hover:bg-card-hover"
            : "text-muted hover:bg-card-hover hover:text-danger",
        visible ? "opacity-100" : "opacity-0 group-hover:opacity-100",
      )}
    >
      {del.isPending ? <Loader2 className="animate-spin" /> : <Trash2 />}
      {armed && (tasks?.length ? `Delete + ${tasks.length}` : "Delete?")}
    </button>
  );
}

function CoreStatus() {
  const { data, isError } = useAppInfo();
  const label = data
    ? `v${data.version} · recording live, history is demo data`
    : isError
      ? "core error"
      : "preview (mock data)";
  return (
    <p className="px-3 pt-1 text-[11px] text-muted/70" title={data?.dataDir}>
      {label}
    </p>
  );
}
