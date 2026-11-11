import { create } from "zustand";
import { commands, events, type Meeting, type Settings_Serialize } from "@/bindings";
import { backend } from "@/lib/mock-backend";
import { call, describeError } from "@/lib/native";
import { isTauri } from "@/lib/platform";
import type { MeetingSummary } from "@/lib/types";
import { useUi } from "./ui";

// Recording session state. In the app it drives the real Rust core and
// listens to `audio://levels` (~10 Hz). In the browser (`pnpm dev`) it fakes
// both. No transcription happens while recording (spec D1).

export type RecordingPhase = "idle" | "recording" | "paused";

const HISTORY_BARS = 56;
const TICK_MS = 100;

interface RecordingState {
  phase: RecordingPhase;
  meetingId: string | null;
  elapsedMs: number;
  levels: { mic: number; system: number }; // 0..1
  history: number[]; // waveform bars, 0..1
  /** Input device ID; "" = system default (app) or a mock ID (browser). */
  micDevice: string;
  /** Message for the user (device fallback, silence, track fault). */
  notice: string | null;
  /** True while a start/stop call is in flight. */
  busy: boolean;
  setMicDevice: (id: string) => void;
  dismissNotice: () => void;
  start: () => Promise<void>;
  pause: () => Promise<void>;
  resume: () => Promise<void>;
  stop: () => Promise<void>;
}

const flat = () => Array.from({ length: HISTORY_BARS }, () => 0);

/** Maps dBFS to a 0..1 meter (−60 dBFS and below read as empty). */
export const meterFromDbfs = (db: number | null | undefined): number =>
  db == null ? 0 : Math.min(1, Math.max(0, (db + 60) / 60));

const defaultTitle = () =>
  `Meeting ${new Date().toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}`;

function toSummary(m: Meeting): MeetingSummary {
  return {
    id: m.id,
    title: m.title,
    language: m.language,
    status: m.status,
    startedAt: m.startedAt ?? Date.now(),
    durationMs: m.durationMs ?? 0,
  };
}

let settings: Settings_Serialize = { micDevice: null, sinkDevice: null, language: "om" };
let ticker: ReturnType<typeof setInterval> | undefined;

/** Current settings as last loaded or saved. */
export const currentSettings = (): Settings_Serialize => settings;

/** Saves a settings change and keeps the recorder's copy in sync. */
export async function saveSettings(patch: Partial<Settings_Serialize>): Promise<void> {
  const next = { ...settings, ...patch };
  settings = await call(commands.settingsSet(next));
  useRecording.setState({ micDevice: settings.micDevice ?? "" });
}

export const useRecording = create<RecordingState>((set, get) => {
  // Browser-only stand-in for `audio://levels`.
  const fakeTick = () => {
    const { phase, levels, history, elapsedMs } = get();
    if (phase !== "recording") {
      set({ levels: { mic: 0, system: 0 }, history: [...history.slice(1), 0] });
      return;
    }
    const wobble = (prev: number, talk: boolean) =>
      Math.min(
        1,
        Math.max(0, prev * 0.35 + (talk ? Math.random() ** 2 * 0.75 : Math.random() * 0.06)),
      );
    const t = elapsedMs / 1000;
    const mic = wobble(levels.mic, Math.sin(t / 3) > 0.1);
    const system = wobble(levels.system, Math.sin(t / 3) < -0.1);
    set({
      elapsedMs: elapsedMs + TICK_MS,
      levels: { mic, system },
      history: [...history.slice(1), Math.max(mic, system)],
    });
  };

  const reset = () =>
    set({
      phase: "idle",
      meetingId: null,
      elapsedMs: 0,
      levels: { mic: 0, system: 0 },
      history: flat(),
    });

  return {
    phase: "idle",
    meetingId: null,
    elapsedMs: 0,
    levels: { mic: 0, system: 0 },
    history: flat(),
    micDevice: isTauri() ? "" : "default",
    notice: null,
    busy: false,

    setMicDevice: (micDevice) => {
      set({ micDevice });
      if (!isTauri()) return;
      saveSettings({ micDevice: micDevice || null }).catch((e) =>
        set({ notice: describeError(e) }),
      );
    },

    dismissNotice: () => set({ notice: null }),

    start: async () => {
      if (get().phase !== "idle" || get().busy) return;
      if (!isTauri()) {
        const meeting = await backend.recordingStart({ language: "om" });
        useUi.getState().selectMeeting(meeting.id);
        set({ phase: "recording", meetingId: meeting.id, elapsedMs: 0, history: flat() });
        clearInterval(ticker);
        ticker = setInterval(fakeTick, TICK_MS);
        return;
      }
      set({ busy: true, notice: null });
      try {
        const res = await call(
          commands.recordingStart({
            title: defaultTitle(),
            language: settings.language,
            micDevice: get().micDevice || null,
            sinkDevice: settings.sinkDevice,
          }),
        );
        backend.upsertRecorded(toSummary(res.meeting), { dir: res.meeting.dir, problems: [] });
        useUi.getState().selectMeeting(res.meeting.id);
        set({
          phase: "recording",
          meetingId: res.meeting.id,
          elapsedMs: 0,
          history: flat(),
          notice: res.notices.length ? res.notices.join(" ") : null,
        });
      } catch (e) {
        set({ notice: describeError(e) });
      } finally {
        set({ busy: false });
      }
    },

    pause: async () => {
      const { phase, meetingId } = get();
      if (phase !== "recording" || !meetingId) return;
      if (isTauri()) {
        try {
          await call(commands.recordingPause(meetingId));
        } catch (e) {
          set({ notice: describeError(e) });
          return;
        }
      }
      set({ phase: "paused" });
    },

    resume: async () => {
      const { phase, meetingId } = get();
      if (phase !== "paused" || !meetingId) return;
      if (isTauri()) {
        try {
          await call(commands.recordingResume(meetingId));
        } catch (e) {
          set({ notice: describeError(e) });
          return;
        }
      }
      set({ phase: "recording" });
    },

    stop: async () => {
      const { meetingId, elapsedMs, busy } = get();
      if (!meetingId || busy) return;
      if (!isTauri()) {
        clearInterval(ticker);
        ticker = undefined;
        reset();
        await backend.recordingStop(meetingId, elapsedMs);
        return;
      }
      set({ busy: true });
      try {
        const res = await call(commands.recordingStop(meetingId));
        backend.upsertRecorded(toSummary(res.meeting), {
          dir: res.meeting.dir,
          problems: res.problems,
          peaks: res.peaks.map((p) => p ?? 0),
        });
        reset();
        if (res.problems.length) set({ notice: res.problems.join(" ") });
      } catch (e) {
        set({ notice: describeError(e) });
      } finally {
        set({ busy: false });
      }
    },
  };
});

/**
 * Connects the store to the core: loads settings and listens to live events,
 * including recordings started or stopped from the tray. Returns a cleanup.
 */
export function connectRecording(): () => void {
  if (!isTauri()) return () => {};
  const unlisten: Promise<() => void>[] = [];
  const store = useRecording;

  call(commands.settingsGet())
    .then((s) => {
      settings = s;
      store.setState({ micDevice: s.micDevice ?? "" });
    })
    .catch((e) => store.setState({ notice: describeError(e) }));

  unlisten.push(
    events.audioLevels.listen(({ payload }) => {
      const st = store.getState();
      if (st.meetingId && payload.meetingId !== st.meetingId) return;
      const mic = meterFromDbfs(payload.mic);
      const system = meterFromDbfs(payload.system);
      store.setState({
        meetingId: payload.meetingId,
        phase: payload.paused ? "paused" : "recording",
        elapsedMs: payload.elapsedMs ?? 0,
        levels: { mic, system },
        history: [...st.history.slice(1), payload.paused ? 0 : Math.max(mic, system)],
      });
    }),
  );

  unlisten.push(
    events.meetingStatus.listen(({ payload }) => {
      const st = store.getState();
      if (payload.status === "recording" && st.meetingId !== payload.meetingId) {
        // Started from the tray: show it like one started here.
        backend.upsertRecorded(
          {
            id: payload.meetingId,
            title: "",
            language: settings.language,
            status: "recording",
            startedAt: Date.now(),
            durationMs: 0,
          },
          { dir: "", problems: [] },
        );
        useUi.getState().selectMeeting(payload.meetingId);
        store.setState({ phase: "recording", meetingId: payload.meetingId, history: flat() });
      } else if (payload.status === "ready" || payload.status === "needs_attention") {
        // Transcription finished (or couldn't run).
        if (st.meetingId !== payload.meetingId)
          backend.updateRecorded(payload.meetingId, { status: payload.status });
      }
      if (payload.status !== "recording" && st.meetingId === payload.meetingId && !st.busy) {
        // Stopped from the tray.
        const m = { id: payload.meetingId, status: payload.status };
        backend
          .meetingGet(m.id)
          .then((d) =>
            backend.upsertRecorded(
              { ...d, status: m.status, durationMs: st.elapsedMs },
              d.recorded ?? { dir: "", problems: [] },
            ),
          )
          .catch(() => {});
        store.setState({
          phase: "idle",
          meetingId: null,
          elapsedMs: 0,
          levels: { mic: 0, system: 0 },
          history: flat(),
        });
      }
    }),
  );

  // Post-stop transcription (in the app, segments come from the core).
  unlisten.push(
    events.transcriptSegments.listen(({ payload }) =>
      backend.appendSegments(
        payload.meetingId,
        payload.segments.map((s) => ({
          id: `${payload.meetingId}-${s.id}`,
          meetingId: payload.meetingId,
          track: s.track,
          speaker: null,
          startMs: s.startMs ?? 0,
          endMs: s.endMs ?? 0,
          text: s.text,
          edited: false,
        })),
      ),
    ),
  );
  unlisten.push(
    events.transcriptProgress.listen(({ payload }) =>
      backend.updateRecorded(payload.meetingId, {
        transcription: {
          done: payload.done,
          failed: payload.failed,
          total: payload.total,
          finished: payload.finished,
          blocked: payload.blocked ?? null,
          message: payload.message ?? null,
        },
      }),
    ),
  );

  unlisten.push(
    events.audioSilent.listen(({ payload }) => {
      const who = payload.track === "mic" ? "your microphone" : "the other participants";
      store.setState({
        notice:
          payload.sinceMs != null
            ? `No sound from ${who} for over 30 seconds. Check the device or mute.`
            : null,
      });
    }),
  );

  unlisten.push(
    events.appError.listen(({ payload }) => store.setState({ notice: payload.message })),
  );

  // A failed subscription (e.g. a missing event permission) would otherwise
  // be an invisible rejected promise and a frozen timer.
  const safe = unlisten.map((u) =>
    u.catch((e: unknown) => {
      store.setState({ notice: `Live recording updates unavailable: ${describeError(e)}` });
      return () => {};
    }),
  );
  return () => {
    for (const u of safe) void u.then((f) => f());
  };
}
