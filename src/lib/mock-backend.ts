// In-memory stand-in for the Rust core (SPEC.md §6.2 commands). The UI talks
// only to `backend`; swapping this for tauri-specta bindings later must not
// require component changes.
//
// Behaviour follows spec D1: nothing is transcribed while recording. Stopping
// a recording moves the meeting to `processing` and segments arrive chunk by
// chunk afterwards.
//
// The Afaan Oromo text below is illustrative sample data only.

import type {
  AudioDevice,
  MeetingDetail,
  MeetingNotes,
  MeetingSummary,
  Segment,
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
          (m.notes?.summary.toLocaleLowerCase().includes(q) ?? false),
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

  /**
   * Adds or updates a meeting recorded by the real core, so it shows in the
   * list until the real store lands (M3). It never gets mock segments.
   */
  upsertRecorded(
    m: MeetingSummary,
    recorded: { dir: string; problems: string[]; peaks?: number[] },
  ): void {
    // Transcription isn't connected yet, so a finished recording is shown as
    // done rather than "Transcribing" forever.
    if (m.status === "processing") m = { ...m, status: "ready" };
    const existing = meetings.find((x) => x.id === m.id);
    if (existing) {
      Object.assign(existing, {
        ...m,
        title: m.title || existing.title,
        recorded: {
          ...existing.recorded,
          ...recorded,
          dir: recorded.dir || existing.recorded?.dir || "",
        },
      });
    } else {
      meetings.push({ ...m, segments: [], notes: null, progress: { done: 0, total: 0 }, recorded });
    }
    emit();
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
