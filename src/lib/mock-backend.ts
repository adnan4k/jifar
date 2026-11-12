// In-memory stand-in for the Rust core (SPEC.md §6.2 commands). The UI talks
// only to `backend`; swapping this for tauri-specta bindings later must not
// require component changes.
//
// Behaviour follows spec D1: nothing is transcribed while recording. Stopping
// a recording moves the meeting to `processing` and segments arrive chunk by
// chunk afterwards.
//
// The Afaan Oromo text below is illustrative sample data only.

import { matchKey } from "./tasks";
import type {
  AudioDevice,
  MeetingDetail,
  MeetingNotes,
  MeetingSummary,
  NewTask,
  Segment,
  Task,
  TaskFilter,
  TaskPatch,
  Track,
} from "./types";

const SCRIPT: { track: Track; text: string }[] = [
  {
    track: "mic",
    text: "Akkam bultan hundi keessan. Har'a karoora kurmaana sadaffaa irratti mari'anna.",
  },
  { track: "system", text: "Galatoomi. Jalqaba rakkoo teeknikaa duraan jiru haa ilaallu." },
  {
    track: "mic",
    text: "Tole. Sirni galmee haaraan yeroo baay'ee ni dhaabata; kun dursa argachuu qaba.",
  },
  {
    track: "system",
    text: "Ani gabaasa rakkoolee sanaa qopheessee har'a galgala siif nan erga.",
  },
  { track: "mic", text: "Gaarii dha. Ammas gabaasni baajataa Jimaata dura xumuramuu qaba." },
  { track: "system", text: "Caalaan gabaasa baajataa Kamisa dura ni qopheessa jedheera." },
  {
    track: "mic",
    text: "Tole, murtee kana galmeessina: hojiin haaraan ji'a itti aanu jalqaba.",
  },
  { track: "system", text: "Waliigalle. Gaaffii biraa qabdaa?" },
  { track: "mic", text: "Lakki, hunduu ifa dha. Galatoomaa, torban dhufu walitti deebina." },
];

const CHUNK_INTERVAL_MS = 900; // simulated ASR latency per chunk

function makeSegments(meetingId: string, count: number): Segment[] {
  const out: Segment[] = [];
  let t = 2_000;
  for (let i = 0; i < count; i++) {
    const line = SCRIPT[i % SCRIPT.length];
    if (!line) continue;
    const len = 3_000 + line.text.length * 60;
    out.push({
      id: `${meetingId}-s${i}`,
      meetingId,
      track: line.track,
      speaker: null,
      startMs: t,
      endMs: t + len,
      text: line.text,
      edited: false,
    });
    t += len + 800;
  }
  return out;
}

function sampleNotes(segments: Segment[]): MeetingNotes {
  const ids = (...idx: number[]) =>
    idx.flatMap((i) => {
      const s = segments[i];
      return s ? [s.id] : [];
    });
  return {
    summary:
      "Walgahiin kun karoora kurmaana sadaffaa, rakkoo teeknikaa fi gabaasa baajataa irratti xiyyeeffate. Hojiin haaraan ji'a itti aanu akka jalqabu murtaa'eera.",
    keyPoints: [
      "Rakkoon sirna galmee haaraa dursa argachuu qaba.",
      "Gabaasni baajataa Jimaata dura xumuramuu qaba.",
      "Torban dhufu walgahiin itti aansu ni taa'a.",
    ],
    decisions: [{ text: "Hojiin haaraan ji'a itti aanu jalqaba.", segmentIds: ids(6) }],
    actionItems: [
      {
        text: "Gabaasa rakkoolee teeknikaa erguu",
        owner: "Others",
        due: "Har'a galgala",
        segmentIds: ids(3),
      },
      {
        text: "Gabaasa baajataa qopheessuu",
        owner: "Caalaa",
        due: "Kamisa",
        segmentIds: ids(5),
      },
    ],
    keywords: ["karoora", "kurmaana 3ffaa", "baajata", "rakkoo teeknikaa", "gabaasa"],
  };
}

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

function seed(): MeetingDetail[] {
  const now = Date.now();
  const mk = (
    title: string,
    ago: number,
    segCount: number,
    status: MeetingDetail["status"],
    withNotes: boolean,
    total = segCount,
  ): MeetingDetail => {
    const id = crypto.randomUUID();
    const segments = makeSegments(id, segCount);
    const last = segments.at(-1);
    return {
      id,
      title,
      language: "om",
      status,
      startedAt: now - ago,
      durationMs: Math.max(last ? last.endMs + 2_000 : 0, 25 * 60_000),
      segments,
      notes: withNotes ? sampleNotes(segments) : null,
      progress: { done: segCount, total },
    };
  };
  return [
    mk("Karoora kurmaana 3ffaa", 2 * HOUR, 9, "ready", true),
    mk("Walgahii garee teeknikaa", DAY + 3 * HOUR, 9, "ready", false),
    mk("Barnoota online: Seenaa", 3 * DAY, 4, "needs_attention", false, 9),
    mk("Marii pirojektii haaraa", 8 * DAY, 9, "ready", true),
  ];
}

const meetings: MeetingDetail[] = seed();

// Tasks (FR-AI-08), seeded from the newest meeting's action items.
type StoredTask = Omit<Task, "meetingTitle" | "sourceStartMs">;
const tasks: StoredTask[] = (() => {
  const m = meetings[0];
  if (!m?.notes) return [];
  return m.notes.actionItems.map((a, i) => ({
    id: crypto.randomUUID(),
    meetingId: m.id,
    text: a.text,
    owner: a.owner ?? null,
    due: a.due ?? null,
    done: i === 0,
    doneAt: i === 0 ? Date.now() : null,
    sourceKind: "action_item",
    sourceVersion: null,
    segmentIds: a.segmentIds,
    matchKey: matchKey(a.text),
    createdAt: Date.now() - HOUR,
  }));
})();

function viewTask(t: StoredTask): Task {
  const m = meetings.find((x) => x.id === t.meetingId);
  const seg = m?.segments.find((s) => s.id === t.segmentIds[0]);
  return { ...t, meetingTitle: m?.title ?? null, sourceStartMs: seg?.startMs ?? null };
}

const clean = (s: string | null | undefined) => s?.trim() || null;
const listeners = new Set<() => void>();
const emit = () => {
  for (const l of listeners) l();
};
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

function find(id: string): MeetingDetail {
  const m = meetings.find((x) => x.id === id);
  if (!m) throw new Error(`meeting_not_found: ${id}`);
  return m;
}

function toSummary(m: MeetingDetail): MeetingSummary {
  const { id, title, language, status, startedAt, durationMs } = m;
  return { id, title, language, status, startedAt, durationMs };
}

/** Post-stop transcription: one segment per simulated ASR chunk (spec §6.4). */
function runProcessing(m: MeetingDetail) {
  m.status = "processing";
  const planned = makeSegments(m.id, m.progress.total);
  emit();
  const timer = setInterval(() => {
    const next = planned[m.progress.done];
    if (next) m.segments.push(next);
    m.progress.done += 1;
    if (m.progress.done >= m.progress.total) {
      clearInterval(timer);
      const last = m.segments.at(-1);
      m.durationMs = Math.max(m.durationMs, last ? last.endMs + 2_000 : 0);
      m.status = "ready";
    }
    emit();
  }, CHUNK_INTERVAL_MS);
}

export const backend = {
  subscribe(fn: () => void): () => void {
    listeners.add(fn);
    return () => {
      listeners.delete(fn);
    };
  },

  async devicesList(): Promise<{ inputs: AudioDevice[]; outputs: AudioDevice[] }> {
    return {
      inputs: [
        { id: "default", label: "Default microphone" },
        { id: "usb-headset", label: "USB Headset" },
        { id: "builtin", label: "Built-in Audio Analog Stereo" },
      ],
      outputs: [{ id: "default", label: "Default output (monitor)" }],
    };
  },

  async meetingsList(query: string): Promise<MeetingSummary[]> {
    const q = query.trim().toLocaleLowerCase();
    return meetings
      .filter(
        (m) =>
          !q ||
          m.title.toLocaleLowerCase().includes(q) ||
          m.segments.some((s) => s.text.toLocaleLowerCase().includes(q)) ||
          (m.notes?.summary.toLocaleLowerCase().includes(q) ?? false) ||
          tasks.some((t) => t.meetingId === m.id && t.text.toLocaleLowerCase().includes(q)),
      )
      .sort((a, b) => b.startedAt - a.startedAt)
      .map(toSummary);
  },

  async meetingGet(id: string): Promise<MeetingDetail> {
    return structuredClone(find(id));
  },

  async recordingStart(input: { title?: string; language: string }): Promise<MeetingSummary> {
    const now = Date.now();
    const m: MeetingDetail = {
      id: crypto.randomUUID(),
      title:
        input.title ??
        `Meeting ${new Date(now).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}`,
      language: input.language,
      status: "recording",
      startedAt: now,
      durationMs: 0,
      segments: [],
      notes: null,
      progress: { done: 0, total: 0 },
    };
    meetings.push(m);
    emit();
    return toSummary(m);
  },

  async recordingStop(id: string, durationMs: number): Promise<MeetingSummary> {
    const m = find(id);
    m.durationMs = durationMs;
    // Real chunking: ~30 s VAD chunks. Capped so the demo finishes quickly.
    m.progress = {
      done: 0,
      total: Math.min(SCRIPT.length, Math.max(3, Math.ceil(durationMs / 30_000))),
    };
    runProcessing(m);
    return toSummary(m);
  },

  async meetingRename(id: string, title: string): Promise<null> {
    find(id).title = title.trim();
    emit();
    return null;
  },

  async meetingDelete(id: string): Promise<null> {
    const i = meetings.findIndex((m) => m.id === id);
    if (i >= 0) meetings.splice(i, 1);
    for (let j = tasks.length - 1; j >= 0; j--) if (tasks[j]?.meetingId === id) tasks.splice(j, 1);
    emit();
    return null;
  },

  async tasksList(filter: TaskFilter, meetingId: string | null, query: string): Promise<Task[]> {
    const q = query.trim().toLocaleLowerCase();
    return tasks
      .filter(
        (t) =>
          (filter === "all" || t.done === (filter === "done")) &&
          (meetingId === null || t.meetingId === meetingId) &&
          (!q || t.text.toLocaleLowerCase().includes(q)),
      )
      .sort((a, b) => Number(a.done) - Number(b.done) || b.createdAt - a.createdAt)
      .map(viewTask);
  },

  async tasksAdd(items: NewTask[]): Promise<Task[]> {
    if (items.some((i) => !i.text.trim())) throw new Error("invalid_input: a task needs some text");
    const now = Date.now();
    const out = items.map((i) => {
      const key = matchKey(i.text);
      const existing = tasks.find(
        (t) =>
          i.sourceKind !== "manual" &&
          i.meetingId !== null &&
          t.meetingId === i.meetingId &&
          t.sourceKind === i.sourceKind &&
          t.matchKey === key,
      );
      if (existing) return existing;
      const t: StoredTask = {
        id: crypto.randomUUID(),
        meetingId: i.meetingId,
        text: i.text.trim(),
        owner: clean(i.owner),
        due: clean(i.due),
        done: false,
        doneAt: null,
        sourceKind: i.sourceKind,
        sourceVersion: i.sourceVersion ?? null,
        segmentIds: i.segmentIds,
        matchKey: key,
        createdAt: now,
      };
      tasks.push(t);
      return t;
    });
    emit();
    return out.map(viewTask);
  },

  async taskUpdate(id: string, patch: TaskPatch): Promise<Task> {
    const t = tasks.find((x) => x.id === id);
    if (!t) throw new Error(`task_not_found: ${id}`);
    if (patch.text !== undefined) {
      if (!patch.text.trim()) throw new Error("invalid_input: a task needs some text");
      t.text = patch.text.trim();
    }
    if (patch.owner !== undefined) t.owner = clean(patch.owner);
    if (patch.due !== undefined) t.due = clean(patch.due);
    if (patch.done !== undefined) {
      t.doneAt = patch.done ? (t.doneAt ?? Date.now()) : null;
      t.done = patch.done;
    }
    emit();
    return viewTask(t);
  },

  async taskDelete(id: string): Promise<null> {
    const i = tasks.findIndex((t) => t.id === id);
    if (i >= 0) tasks.splice(i, 1);
    emit();
    return null;
  },

  async tasksCounts(): Promise<{ open: number; done: number }> {
    const done = tasks.filter((t) => t.done).length;
    return { open: tasks.length - done, done };
  },

  async queueRetry(id: string): Promise<void> {
    runProcessing(find(id));
  },

  async notesGenerate(id: string): Promise<MeetingNotes> {
    await delay(1_500);
    const m = find(id);
    m.notes = sampleNotes(m.segments);
    emit();
    return structuredClone(m.notes);
  },
};
