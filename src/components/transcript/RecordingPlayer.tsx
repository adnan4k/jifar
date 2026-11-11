import { convertFileSrc } from "@tauri-apps/api/core";
import { AlertTriangle, Pause, Play } from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { isTauri } from "@/lib/platform";
import { cn, formatTimestamp } from "@/lib/utils";

const BARS = 120;

/**
 * Plays a real recording's `mixed.opus` through the `jifar-audio://` scheme
 * and shows its waveform; click the waveform to seek. Render it with
 * `key={meetingId}` so switching meetings starts fresh.
 */
export function RecordingPlayer({
  meetingId,
  durationMs,
  peaks,
}: {
  meetingId: string;
  durationMs: number;
  peaks: number[] | undefined;
}) {
  const audio = useRef<HTMLAudioElement>(null);
  const [playing, setPlaying] = useState(false);
  const [positionMs, setPositionMs] = useState(0);
  const [failed, setFailed] = useState(false);
  const [mediaMs, setMediaMs] = useState<number | null>(null);
  const src = useMemo(
    () => (isTauri() ? convertFileSrc(`${meetingId}/mixed.opus`, "jifar-audio") : undefined),
    [meetingId],
  );
  const totalMs = mediaMs ?? durationMs;
  const bars = useMemo(() => resample(peaks ?? [], BARS), [peaks]);
  const progress = totalMs > 0 ? positionMs / totalMs : 0;

  const toggle = () => {
    const el = audio.current;
    if (!el) return;
    if (el.paused) {
      el.play().catch(() => setFailed(true));
    } else {
      el.pause();
    }
  };

  const seekTo = (fraction: number) => {
    const el = audio.current;
    if (!el || totalMs <= 0) return;
    el.currentTime = (fraction * totalMs) / 1000;
    setPositionMs(fraction * totalMs);
  };

  if (!src) return null;

  return (
    <div className="w-full space-y-3 rounded-xl bg-card p-4">
      {/* biome-ignore lint/a11y/useMediaCaption: meeting audio has no caption track; the transcript is the text alternative */}
      <audio
        ref={audio}
        src={src}
        preload="metadata"
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => setPlaying(false)}
        onTimeUpdate={(e) => setPositionMs(e.currentTarget.currentTime * 1000)}
        onLoadedMetadata={(e) => {
          const d = e.currentTarget.duration;
          if (Number.isFinite(d) && d > 0) setMediaMs(d * 1000);
        }}
        onError={() => setFailed(true)}
      />
      <button
        type="button"
        aria-label="Seek"
        className="flex h-16 w-full cursor-pointer items-center gap-[2px]"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          seekTo(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)));
        }}
      >
        {bars.map((v, i) => (
          <span
            // biome-ignore lint/suspicious/noArrayIndexKey: fixed-length bar strip
            key={i}
            className={cn(
              "flex-1 rounded-full transition-colors",
              i / bars.length < progress ? "bg-accent" : "bg-muted/40",
            )}
            style={{ height: `${Math.max(6, v * 100)}%` }}
          />
        ))}
      </button>
      <div className="flex items-center gap-3">
        <Button
          size="icon"
          className="rounded-full"
          onClick={toggle}
          disabled={failed}
          aria-label={playing ? "Pause" : "Play"}
        >
          {playing ? <Pause /> : <Play />}
        </Button>
        <span className="text-xs text-muted tabular-nums">
          {formatTimestamp(positionMs)} / {formatTimestamp(totalMs)}
        </span>
        {failed && (
          <span className="flex items-center gap-1.5 text-xs text-warn">
            <AlertTriangle className="size-3.5" />
            Can't play this audio here. Ogg/Opus playback needs the GStreamer base and good plugins.
          </span>
        )}
      </div>
    </div>
  );
}

/** Max-pools (or pads) `peaks` to `n` bars. Flat bars if there's no data. */
function resample(peaks: number[], n: number): number[] {
  if (peaks.length === 0) return Array.from({ length: n }, () => 0.08);
  return Array.from({ length: n }, (_, b) => {
    const lo = Math.floor((b * peaks.length) / n);
    const hi = Math.max(lo + 1, Math.floor(((b + 1) * peaks.length) / n));
    return Math.max(...peaks.slice(lo, hi));
  });
}
