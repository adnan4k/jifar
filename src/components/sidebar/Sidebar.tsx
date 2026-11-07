import { AudioLines, Search, Settings } from "lucide-react";
import { StatusBadge } from "@/components/ui/status-badge";
import { useAppInfo } from "@/lib/native";
import { useMeetings } from "@/lib/queries";
import { cn, formatDate, formatDuration } from "@/lib/utils";
import { useUi } from "@/stores/ui";
import { DevicePicker, RecordControls, Waveform } from "./RecordControls";

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
      </div>

      <MeetingList />

      <div className="border-t p-3">
        <button
          type="button"
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
  const { search, setSearch, selectedMeetingId, selectMeeting } = useUi();
  const { data: meetings = [] } = useMeetings(search);

  return (
    <div className="mt-4 flex min-h-0 flex-1 flex-col border-t pt-4">
      <label className="relative mx-4 mb-3 block">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted" />
        <input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search meetings…"
          className="h-9 w-full rounded-lg border bg-bg pr-3 pl-8 text-sm outline-none placeholder:text-muted focus-visible:ring-2 focus-visible:ring-accent/60"
        />
      </label>

      <p className="px-5 pb-1.5 text-xs font-medium tracking-wide text-muted uppercase">Meetings</p>
      <ul className="min-h-0 flex-1 space-y-0.5 overflow-y-auto px-2 pb-2">
        {meetings.length === 0 && (
          <li className="px-3 py-6 text-center text-sm text-muted">
            {search ? "No matches" : "No meetings yet"}
          </li>
        )}
        {meetings.map((m) => (
          <li key={m.id}>
            <button
              type="button"
              onClick={() => selectMeeting(m.id)}
              className={cn(
                "w-full rounded-lg px-3 py-2 text-left transition-colors hover:bg-card-hover",
                selectedMeetingId === m.id && "bg-card",
              )}
            >
              <div className="truncate text-sm">{m.title}</div>
              <div className="mt-0.5 flex items-center gap-2 text-xs text-muted">
                <span>{formatDate(m.startedAt)}</span>
                {m.status !== "recording" && <span>· {formatDuration(m.durationMs)}</span>}
                <StatusBadge status={m.status} className="ml-auto" />
              </div>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function CoreStatus() {
  const { data, isError } = useAppInfo();
  const label = data ? `v${data.version}` : isError ? "core error" : "preview (mock data)";
  return (
    <p className="px-3 pt-1 text-[11px] text-muted/70" title={data?.dataDir}>
      {label}
    </p>
  );
}
