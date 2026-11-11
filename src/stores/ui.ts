import { create } from "zustand";

interface UiState {
  selectedMeetingId: string | null;
  search: string;
  /** Shared playhead so transcript, notes links and player stay in sync. */
  playheadMs: number;
  /** Bumped by `seek` so the audio player jumps (not by playback itself). */
  seekNonce: number;
  settingsOpen: boolean;
  setSettingsOpen: (open: boolean) => void;
  selectMeeting: (id: string | null) => void;
  setSearch: (q: string) => void;
  seek: (ms: number) => void;
}

export const useUi = create<UiState>((set) => ({
  selectedMeetingId: null,
  search: "",
  playheadMs: 0,
  seekNonce: 0,
  settingsOpen: false,
  setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
  selectMeeting: (id) => set({ selectedMeetingId: id, playheadMs: 0 }),
  setSearch: (search) => set({ search }),
  seek: (playheadMs) => set((s) => ({ playheadMs, seekNonce: s.seekNonce + 1 })),
}));
