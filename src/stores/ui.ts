import { create } from "zustand";

/** What the centre column shows. */
export type View = "meetings" | "tasks";

interface UiState {
  view: View;
  selectedMeetingId: string | null;
  search: string;
  /** Shared playhead so transcript, notes links and player stay in sync. */
  playheadMs: number;
  /** Bumped by `seek` so the audio player jumps (not by playback itself). */
  seekNonce: number;
  settingsOpen: boolean;
  setView: (view: View) => void;
  setSettingsOpen: (open: boolean) => void;
  /** Opening a meeting leaves the Tasks view. */
  selectMeeting: (id: string | null) => void;
  setSearch: (q: string) => void;
  seek: (ms: number) => void;
  /** Opens a meeting at a point in its recording (task "jump to source"). */
  openAt: (meetingId: string, ms: number | null) => void;
}

export const useUi = create<UiState>((set, get) => ({
  view: "meetings",
  selectedMeetingId: null,
  search: "",
  playheadMs: 0,
  seekNonce: 0,
  settingsOpen: false,
  setView: (view) => set({ view }),
  setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
  selectMeeting: (id) =>
    set((s) => ({
      selectedMeetingId: id,
      playheadMs: 0,
      view: id === null ? s.view : "meetings",
    })),
  setSearch: (search) => set({ search }),
  seek: (playheadMs) => set((s) => ({ playheadMs, seekNonce: s.seekNonce + 1 })),
  openAt: (meetingId, ms) => {
    get().selectMeeting(meetingId);
    if (ms !== null) get().seek(ms);
  },
}));
