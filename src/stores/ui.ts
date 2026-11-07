import { create } from "zustand";

interface UiState {
  selectedMeetingId: string | null;
  search: string;
  /** Shared playhead so transcript, notes links and player stay in sync. */
  playheadMs: number;
  selectMeeting: (id: string | null) => void;
  setSearch: (q: string) => void;
  seek: (ms: number) => void;
}

export const useUi = create<UiState>((set) => ({
  selectedMeetingId: null,
  search: "",
  playheadMs: 0,
  selectMeeting: (id) => set({ selectedMeetingId: id, playheadMs: 0 }),
  setSearch: (search) => set({ search }),
  seek: (playheadMs) => set({ playheadMs }),
}));
