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
  /** Saved version and its language (real meetings). */
  version?: number;
  language?: string;
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
  recorded?: {
    dir: string;
    problems: string[];
    peaks?: number[];
  };
}

/** Live post-stop transcription progress (events from the core). */
export interface TranscriptionState {
  done: number;
  failed: number;
  total: number;
  finished: boolean;
  /** Paused until the user fixes the API key or balance. */
  blocked: "auth" | "credits" | null;
  message: string | null;
}

export interface AudioDevice {
  id: string;
  label: string;
  isDefault?: boolean;
}

/** Tasks from notes (FR-AI-08, spec D12). */
export type TaskSourceKind =
  | "action_item"
  | "decision"
  | "key_point"
  | "summary"
  | "transcript"
  | "manual";

export type TaskFilter = "open" | "done" | "all";

export interface Task {
  id: string;
  /** null for a task typed by hand without a meeting. */
  meetingId: string | null;
  meetingTitle: string | null;
  text: string;
  owner: string | null;
  due: string | null;
  done: boolean;
  doneAt: number | null; // epoch ms
  sourceKind: TaskSourceKind;
  sourceVersion: number | null;
  segmentIds: string[];
  /** Where "jump to source" seeks to, if the segment still exists. */
  sourceStartMs: number | null;
  /** See `matchKey` in lib/tasks.ts. */
  matchKey: string;
  createdAt: number; // epoch ms
}

export interface NewTask {
  meetingId: string | null;
  text: string;
  owner?: string | null;
  due?: string | null;
  sourceKind: TaskSourceKind;
  sourceVersion?: number | null;
  segmentIds: string[];
}

/** Omitted fields stay; an empty owner/due clears it. */
export interface TaskPatch {
  text?: string;
  owner?: string;
  due?: string;
  done?: boolean;
}
