import { convertFileSrc } from "@tauri-apps/api/core";
import { AlertTriangle, Loader2, Pause, Play } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { isTauri } from "@/lib/platform";
import { cn, formatTimestamp } from "@/lib/utils";
import { useUi } from "@/stores/ui";

const BARS = 120;

/**
 * Plays a real recording's `mixed.opus` and shows its waveform; click the
 * waveform to seek. Render it with `key={meetingId}` so switching meetings
 * starts fresh.
 *
 * The file is fetched from the `jifar-audio://` scheme and played from a
 * `blob:` URL: WebKitGTK's media pipeline can't read custom schemes directly.
 */
export function RecordingPlayer({
  meetingId,
  durationMs,
  peaks,
  compact = false,
}: {
  meetingId: string;
  durationMs: number;
  peaks: number[] | undefined;
  /** Shorter waveform, for the bar under the transcript. */
  compact?: boolean;
}) {
  const audio = useRef<HTMLAudioElement>(null);
  const [playing, setPlaying] = useState(false);
  const [positionMs, setPositionMs] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [mediaMs, setMediaMs] = useState<number | null>(null);
  const [blobUrl, setBlobUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri()) return;
    let url: string | null = null;
    let cancelled = false;
    fetch(convertFileSrc(`${meetingId}/mixed.opus`, "jifar-audio"))
      .then(async (res) => {
        if (!res.ok) throw new Error(`the audio file could not be read (HTTP ${res.status})`);
        const blob = new Blob([await res.arrayBuffer()], { type: "audio/ogg" });
        if (cancelled) return;
        url = URL.createObjectURL(blob);
        setBlobUrl(url);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [meetingId]);
  const totalMs = mediaMs ?? durationMs;
  const seekNonce = useUi((s) => s.seekNonce);

  // Clicking a transcript segment seeks the audio there.
  useEffect(() => {
    const el = audio.current;
    if (!el || seekNonce === 0) return;
    el.currentTime = useUi.getState().playheadMs / 1000;
    setPositionMs(useUi.getState().playheadMs);
  }, [seekNonce]);
  const bars = useMemo(() => resample(peaks ?? [], BARS), [peaks]);
  const progress = totalMs > 0 ? positionMs / totalMs : 0;

  const toggle = () => {
    const el = audio.current;
    if (!el) return;
    if (el.paused) {
      el.play().catch((e: unknown) =>
        setError(`playback was refused: ${e instanceof Error ? e.message : String(e)}`),
      );
    } else {
      el.pause();
    }
  };

  const seekTo = (fraction: number) => {
    const el = audio.current;
    if (!el || totalMs <= 0) return;
    el.currentTime = (fraction * totalMs) / 1000;
    setPositionMs(fraction * totalMs);
    useUi.setState({ playheadMs: fraction * totalMs });
  };

  if (!isTauri()) return null;

  return (
    <div className={cn("w-full", compact ? "space-y-2" : "space-y-3 rounded-xl bg-card p-4")}>
      {/* biome-ignore lint/a11y/useMediaCaption: meeting audio has no caption track; the transcript is the text alternative */}
      <audio
        ref={audio}
        src={blobUrl ?? undefined}
        preload="metadata"
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => setPlaying(false)}
        onTimeUpdate={(e) => {
          const ms = e.currentTarget.currentTime * 1000;
          setPositionMs(ms);
          // Highlights the segment being played (without seeking again).
          useUi.setState({ playheadMs: ms });
        }}
        onLoadedMetadata={(e) => {
          const d = e.currentTarget.duration;
          if (Number.isFinite(d) && d > 0) setMediaMs(d * 1000);
        }}
        onError={(e) => setError(mediaErrorText(e.currentTarget.error))}
      />
      <button
        type="button"
        aria-label="Seek"
        className={cn(
          "flex w-full cursor-pointer items-center gap-[2px]",
          compact ? "h-9" : "h-16",
        )}
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
          disabled={!blobUrl || error !== null}
          aria-label={playing ? "Pause" : "Play"}
        >
          {!blobUrl && !error ? (
            <Loader2 className="animate-spin" />
          ) : playing ? (
            <Pause />
          ) : (
            <Play />
          )}
        </Button>
        <span className="text-xs text-muted tabular-nums">
          {formatTimestamp(positionMs)} / {formatTimestamp(totalMs)}
        </span>
        {error && (
          <span className="flex items-center gap-1.5 text-left text-xs text-warn">
            <AlertTriangle className="size-3.5 shrink-0" />
            Can't play this recording: {error}
          </span>
        )}
      </div>
    </div>
  );
}

function mediaErrorText(err: MediaError | null): string {
  switch (err?.code) {
    case MediaError.MEDIA_ERR_DECODE:
      return "the audio could not be decoded (GStreamer Opus support missing?)";
    case MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED:
      return "Ogg/Opus isn't supported here (install gstreamer1.0-plugins-base and -good)";
    case MediaError.MEDIA_ERR_NETWORK:
      return "the audio stopped loading";
    default:
      return err?.message || "unknown media error";
  }
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
