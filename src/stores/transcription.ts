import { create } from "zustand";
import type { TranscriptionState } from "@/lib/types";

// Live transcription progress per meeting, from `transcript://progress`.
// Not stored in the database: after a restart only the meeting status and
// the saved segments remain, which is what the UI needs.

interface TranscriptionStore {
  byMeeting: Record<string, TranscriptionState>;
  set: (meetingId: string, state: TranscriptionState) => void;
}

export const useTranscriptionStore = create<TranscriptionStore>((set) => ({
  byMeeting: {},
  set: (meetingId, state) => set((s) => ({ byMeeting: { ...s.byMeeting, [meetingId]: state } })),
}));

/** Live progress for a meeting, if a transcription ran in this session. */
export const useTranscription = (meetingId: string): TranscriptionState | undefined =>
  useTranscriptionStore((s) => s.byMeeting[meetingId]);
