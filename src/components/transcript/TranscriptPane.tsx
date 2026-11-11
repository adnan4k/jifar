import {
  AlertTriangle,
  CheckCircle2,
  Loader2,
  Mic,
  Pause,
  Play,
  RotateCw,
  Settings,
} from "lucide-react";
import { type Ref, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { useMeeting, useRetryQueue } from "@/lib/queries";
import type { MeetingDetail, Segment } from "@/lib/types";
import { cn, formatTimestamp } from "@/lib/utils";
import { useRecording } from "@/stores/recording";
import { useUi } from "@/stores/ui";
import { RecordingPlayer } from "./RecordingPlayer";

export function TranscriptPane() {
  const selectedId = useUi((s) => s.selectedMeetingId);
  const { data: meeting } = useMeeting(selectedId);

  return (
    <main className="flex min-w-0 flex-1 flex-col">
      <header className="flex h-14 shrink-0 items-center justify-between border-b px-6">
        <h1 className="truncate font-medium">{meeting?.title ?? "Meeting transcription"}</h1>
        <Button variant="outline" size="sm">
          <Settings />
          Settings
        </Button>
      </header>

      {!meeting ? <EmptyState /> : <MeetingBody meeting={meeting} />}
    </main>
  );
}

function EmptyState() {
  return (
    <div className="grid flex-1 place-items-center p-8 text-center">
      <div className="max-w-sm space-y-3">
        <div className="mx-auto grid size-14 place-items-center rounded-full bg-accent-soft">
          <Mic className="size-6 text-accent" />
        </div>
        <h2 className="text-lg font-medium">Press record to capture your meeting</h2>
        <p className="text-sm text-muted">
          Jifar records your microphone and the other participants' audio. The transcript is created
          after you stop. Everything stays on this computer.
        </p>
      </div>
    </div>
  );
}

function MeetingBody({ meeting }: { meeting: MeetingDetail }) {
  if (meeting.status === "recording") return <RecordingPlaceholder />;
  if (meeting.recorded) return <RecordedPanel meeting={meeting} />;

  return (
    <>
      {meeting.status === "processing" && <ProcessingBar meeting={meeting} />}
      {meeting.status === "needs_attention" && <AttentionBanner meeting={meeting} />}
      <SegmentList meeting={meeting} />
      {meeting.status !== "processing" && <PlayerBar durationMs={meeting.durationMs} />}
    </>
  );
}

// Spec D1: no live transcript. Show capture feedback only.
function RecordingPlaceholder() {
  const { phase, levels } = useRecording();
  return (
    <div className="grid flex-1 place-items-center p-8 text-center">
      <div className="w-full max-w-sm space-y-6">
        <div className="space-y-2">
          <h2 className="text-lg font-medium">
            {phase === "paused" ? "Recording paused" : "Recording…"}
          </h2>
          <p className="text-sm text-muted">The transcript will appear after you stop.</p>
        </div>
        <div className="space-y-3 text-left">
          <LevelMeter label="Me" value={levels.mic} />
          <LevelMeter label="Others" value={levels.system} />
        </div>
      </div>
    </div>
  );
}

// A real recording from the core. Transcription isn't wired yet (M2/M3), so
// show what was saved instead of a fake transcript.
function RecordedPanel({ meeting }: { meeting: MeetingDetail }) {
  const problems = meeting.recorded?.problems ?? [];
  return (
    <div className="grid flex-1 place-items-center p-8 text-center">
      <div className="w-full max-w-xl space-y-4">
        <div className="mx-auto grid size-14 place-items-center rounded-full bg-accent-soft">
          {problems.length ? (
            <AlertTriangle className="size-6 text-warn" />
          ) : (
            <CheckCircle2 className="size-6 text-accent" />
          )}
        </div>
        <h2 className="text-lg font-medium">Recording saved</h2>
        <p className="text-sm text-muted">
          {formatTimestamp(meeting.durationMs)} of audio, recorded on this computer. Transcription
          is not connected yet; it arrives with the transcription milestone.
        </p>
        <RecordingPlayer
          key={meeting.id}
          meetingId={meeting.id}
          durationMs={meeting.durationMs}
          peaks={meeting.recorded?.peaks}
        />
        {meeting.recorded?.dir && (
          <p className="break-all rounded-lg bg-card px-3 py-2 font-mono text-xs text-muted">
            {meeting.recorded.dir}
          </p>
        )}
        {problems.length > 0 && (
          <ul className="space-y-1 text-left text-sm text-warn">
            {problems.map((p) => (
              <li key={p}>• {p}</li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function LevelMeter({ label, value }: { label: string; value: number }) {
  return (
    <div className="flex items-center gap-3">
      <span className="w-14 text-xs text-muted">{label}</span>
      <div className="h-2 flex-1 overflow-hidden rounded-full bg-card">
        <div
          className="h-full rounded-full bg-accent transition-[width] duration-100"
          style={{ width: `${value * 100}%` }}
        />
      </div>
    </div>
  );
}

function ProcessingBar({ meeting }: { meeting: MeetingDetail }) {
  const { done, total } = meeting.progress;
  return (
    <div className="border-b px-6 py-3">
      <div className="mb-2 flex items-center gap-2 text-sm">
        <Loader2 className="size-4 animate-spin text-accent" />
        Transcribing {done} / {total} chunks
      </div>
      <div className="h-1 overflow-hidden rounded-full bg-card">
        <div
          className="h-full bg-accent transition-[width]"
          style={{ width: `${total ? (done / total) * 100 : 0}%` }}
        />
      </div>
    </div>
  );
}

function AttentionBanner({ meeting }: { meeting: MeetingDetail }) {
  const retry = useRetryQueue();
  const { done, total } = meeting.progress;
  return (
    <div className="flex items-center gap-3 border-b bg-warn/10 px-6 py-3 text-sm">
      <AlertTriangle className="size-4 shrink-0 text-warn" />
      <span className="flex-1">
        {total - done} of {total} chunks could not be transcribed. Check your connection and try
        again.
      </span>
      <Button size="sm" variant="outline" onClick={() => retry.mutate(meeting.id)}>
        <RotateCw />
        Retry
      </Button>
    </div>
  );
}

function SegmentList({ meeting }: { meeting: MeetingDetail }) {
  const { playheadMs, seek } = useUi();
  const active = meeting.segments.find((s) => playheadMs >= s.startMs && playheadMs < s.endMs);
  const activeRef = useRef<HTMLButtonElement>(null);
  const activeId = active?.id;

  useEffect(() => {
    if (activeId) activeRef.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [activeId]);

  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
      <div className="mx-auto max-w-3xl space-y-3">
        {meeting.segments.map((s) => (
          <SegmentCard
            key={s.id}
            segment={s}
            active={s.id === activeId}
            ref={s.id === activeId ? activeRef : undefined}
            onClick={() => seek(s.startMs)}
          />
        ))}
        {meeting.status === "processing" && (
          <div className="h-20 animate-pulse rounded-xl bg-card/60" />
        )}
      </div>
    </div>
  );
}

function SegmentCard({
  segment,
  active,
  onClick,
  ref,
}: {
  segment: Segment;
  active: boolean;
  onClick: () => void;
  ref?: Ref<HTMLButtonElement>;
}) {
  const label = segment.speaker ?? (segment.track === "mic" ? "Me" : "Others");
  return (
    <button
      ref={ref}
      type="button"
      onClick={onClick}
      className={cn(
        "block w-full rounded-xl border border-transparent bg-card px-5 py-4 text-left transition-colors hover:bg-card-hover",
        active && "border-accent/60 bg-card-hover",
      )}
    >
      <div className="mb-1.5 flex items-baseline gap-2 text-sm">
        <span className={cn("font-medium", segment.track === "mic" ? "text-accent" : "text-fg")}>
          {label}
        </span>
        <span className="text-xs text-muted tabular-nums">{formatTimestamp(segment.startMs)}</span>
      </div>
      <p className="leading-relaxed text-fg/90">{segment.text}</p>
    </button>
  );
}

// Mock player: real playback lands with the audio URI scheme (PLAN.md §6.2).
function PlayerBar({ durationMs }: { durationMs: number }) {
  const { playheadMs, seek } = useUi();
  const [playing, setPlaying] = useState(false);

  useEffect(() => {
    if (!playing) return;
    const t = setInterval(() => {
      const next = useUi.getState().playheadMs + 250;
      if (next >= durationMs) setPlaying(false);
      seek(Math.min(next, durationMs));
    }, 250);
    return () => clearInterval(t);
  }, [playing, durationMs, seek]);

  return (
    <div className="flex shrink-0 items-center gap-4 border-t bg-panel px-6 py-3">
      <Button
        size="icon"
        className="rounded-full"
        onClick={() => setPlaying((p) => !p)}
        aria-label={playing ? "Pause" : "Play"}
      >
        {playing ? <Pause /> : <Play />}
      </Button>
      <span className="w-12 text-xs text-muted tabular-nums">{formatTimestamp(playheadMs)}</span>
      <input
        type="range"
        min={0}
        max={durationMs}
        step={250}
        value={playheadMs}
        onChange={(e) => seek(Number(e.target.value))}
        aria-label="Seek"
        className="flex-1 accent-accent"
      />
      <span className="w-12 text-right text-xs text-muted tabular-nums">
        {formatTimestamp(durationMs)}
      </span>
    </div>
  );
}
