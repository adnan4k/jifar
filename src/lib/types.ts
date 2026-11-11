// Domain types mirroring SPEC.md §6.2 / §7.2. These will be replaced by the
// tauri-specta generated bindings once the Rust core exists.

export type LangCode = string; // ISO 639-1, Afaan Oromo = "om"

export type MeetingStatus = "recording" | "processing" | "ready" | "needs_attention";

export type Track = "mic" | "system";

export interface MeetingSummary {
  id: string;
  title: string;
  language: LangCode;
  status: MeetingStatus;
  startedAt: number; // epoch ms
  durationMs: number;
}

export interface Segment {
  id: string;
  meetingId: string;
  track: Track;
  speaker: string | null; // user-renamed label (FR-TRN-08), null = default Me/Others
  startMs: number; // relative to recording start
  endMs: number;
  text: string;
  edited: boolean;
}

export interface MeetingNotes {
  summary: string;
  keyPoints: string[];
  decisions: { text: string; segmentIds: string[] }[];
  actionItems: { text: string; owner?: string; due?: string; segmentIds: string[] }[];
  /** Proposed FR-AI-07 (see PLAN.md §6). Not in the spec yet. */
  keywords: string[];
}

export interface MeetingDetail extends MeetingSummary {
  segments: Segment[];
  notes: MeetingNotes | null;
  /** ASR chunk progress while processing. */
  progress: { done: number; total: number };
  /**
   * Set for meetings recorded by the real Rust core. Until the store and
   * queue exist (M3) they have audio on disk but no transcript yet.
   */
  recorded?: { dir: string; problems: string[]; peaks?: number[] };
}

export interface AudioDevice {
  id: string;
  label: string;
  isDefault?: boolean;
}
