import { create } from "zustand";
import { backend } from "@/lib/mock-backend";
import { useUi } from "./ui";

// Recording session state. The ticker fakes `audio://levels` (~10 Hz) until the
// Rust audio core exists. No transcription happens here (spec D1).

export type RecordingPhase = "idle" | "recording" | "paused";

const HISTORY_BARS = 56;
const TICK_MS = 100;

interface RecordingState {
  phase: RecordingPhase;
  meetingId: string | null;
  elapsedMs: number;
  levels: { mic: number; system: number }; // 0..1
  history: number[]; // waveform bars, 0..1
  micDevice: string;
  setMicDevice: (id: string) => void;
  start: () => Promise<void>;
  pause: () => void;
  resume: () => void;
  stop: () => Promise<void>;
}

let ticker: ReturnType<typeof setInterval> | undefined;

const flat = () => Array.from({ length: HISTORY_BARS }, () => 0);

export const useRecording = create<RecordingState>((set, get) => {
  const tick = () => {
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

  return {
    phase: "idle",
    meetingId: null,
    elapsedMs: 0,
    levels: { mic: 0, system: 0 },
    history: flat(),
    micDevice: "default",
    setMicDevice: (micDevice) => set({ micDevice }),

    start: async () => {
      if (get().phase !== "idle") return;
      const meeting = await backend.recordingStart({ language: "om" });
      useUi.getState().selectMeeting(meeting.id);
      set({ phase: "recording", meetingId: meeting.id, elapsedMs: 0, history: flat() });
      clearInterval(ticker);
      ticker = setInterval(tick, TICK_MS);
    },

    pause: () => {
      if (get().phase === "recording") set({ phase: "paused" });
    },

    resume: () => {
      if (get().phase === "paused") set({ phase: "recording" });
    },

    stop: async () => {
      const { meetingId, elapsedMs } = get();
      if (!meetingId) return;
      clearInterval(ticker);
      ticker = undefined;
      set({
        phase: "idle",
        meetingId: null,
        elapsedMs: 0,
        levels: { mic: 0, system: 0 },
        history: flat(),
      });
      await backend.recordingStop(meetingId, elapsedMs);
    },
  };
});
