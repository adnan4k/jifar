import { Mic, Pause, Play, Square } from "lucide-react";
import { useDevices } from "@/lib/queries";
import { cn, formatClock } from "@/lib/utils";
import { useRecording } from "@/stores/recording";

export function RecordControls() {
  const { phase, start, pause, resume, stop } = useRecording();
  const active = phase !== "idle";

  return (
    <div className="flex items-center justify-center gap-5">
      <button
        type="button"
        onClick={phase === "paused" ? resume : pause}
        disabled={!active}
        title={phase === "paused" ? "Resume" : "Pause"}
        className="grid size-12 place-items-center rounded-full bg-card text-muted transition-colors hover:text-fg disabled:opacity-40"
      >
        {phase === "paused" ? <Play className="size-5" /> : <Pause className="size-5" />}
      </button>

      <button
        type="button"
        onClick={() => void start()}
        disabled={active}
        title={active ? "Recording" : "Start recording"}
        className={cn(
          "grid size-[72px] place-items-center rounded-full bg-accent text-white transition-all",
          "ring-4 ring-accent/25 hover:bg-accent/90 disabled:cursor-default",
          phase === "recording" && "animate-[pulse_2s_ease-in-out_infinite] ring-8 ring-accent/30",
        )}
      >
        <Mic className="size-8" />
      </button>

      <button
        type="button"
        onClick={() => void stop()}
        disabled={!active}
        title="Stop and transcribe"
        className="grid size-12 place-items-center rounded-full border bg-panel transition-colors hover:bg-card-hover disabled:opacity-40"
      >
        <Square className="size-4 fill-danger text-danger" />
      </button>
    </div>
  );
}

export function DevicePicker() {
  const { data } = useDevices();
  const { micDevice, setMicDevice, phase } = useRecording();
  return (
    <select
      value={micDevice}
      onChange={(e) => setMicDevice(e.target.value)}
      disabled={phase !== "idle"}
      aria-label="Microphone"
      className="mx-auto block h-8 max-w-[200px] truncate rounded-md border bg-panel px-2 text-xs text-fg outline-none focus-visible:ring-2 focus-visible:ring-accent/60 disabled:opacity-60"
    >
      {data?.inputs.map((d) => (
        <option key={d.id} value={d.id}>
          {d.label}
        </option>
      ))}
    </select>
  );
}

export function Waveform() {
  const { history, phase, elapsedMs } = useRecording();
  return (
    <div className="flex flex-col items-center gap-2">
      <div className="flex h-24 w-full items-center justify-center gap-[3px]" aria-hidden>
        {history.map((v, i) => (
          <span
            // biome-ignore lint/suspicious/noArrayIndexKey: fixed-length bar strip
            key={i}
            className="w-[3px] rounded-full bg-accent transition-[height] duration-100"
            style={{ height: `${Math.max(4, v * 100)}%`, opacity: phase === "idle" ? 0.35 : 0.9 }}
          />
        ))}
      </div>
      <div className="flex h-5 items-center gap-2 text-sm tabular-nums">
        {phase === "recording" && <span className="size-2 animate-pulse rounded-full bg-danger" />}
        {phase === "paused" && <span className="text-xs text-warn">Paused</span>}
        <span className={phase === "idle" ? "text-muted" : "text-fg"}>
          {formatClock(elapsedMs)}
        </span>
      </div>
    </div>
  );
}
