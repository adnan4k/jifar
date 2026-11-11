# Jifar — V1 Specification

> **Principle:** Jifar should make it effortless to turn spoken meetings into useful, searchable knowledge — starting with Afaan Oromo — while keeping the experience private and affordable.

| Field   | Value                                  |
| ------- | -------------------------------------- |
| Status  | Draft v0.3 (post-meeting transcription only; UI design in [`DESIGN.md`](./DESIGN.md)) |
| Date    | 2026-11-07                             |
| Scope   | V1, Linux desktop (Ubuntu 22.04+ first) |
| Owner   | Jifar core team                        |

Requirement IDs (`FR-*`, `NFR-*`) are stable. Reference them in commits, issues and tests.

---

## 1. Problem

- People can't take good notes while also taking part in a meeting.
- Decisions and action items get forgotten.
- Reviewing long recordings takes too long.
- Afaan Oromo speakers have few good AI meeting tools.
- Existing assistants depend on expensive cloud services and upload private audio.

## 2. Users

V1 targets **individual users**, with no team features:

- People who attend online meetings regularly.
- Students in online classes and study sessions.
- Remote professionals.
- Teams that hold meetings in Afaan Oromo.
- Anyone who wants searchable records of their conversations.

## 3. Settled decisions

These answer the open questions in the product scope. Each one can be revisited, but changing one means updating this section first.

| #  | Decision               | Choice                                                                 | Rationale |
| -- | ---------------------- | ---------------------------------------------------------------------- | --------- |
| D1 | When to transcribe     | **After the meeting only** (V1). Transcription starts when recording stops, with queued recovery. Near-live transcription is deferred (FR-TRN-03) | Simpler, lower-risk recording pipeline: no VAD/ASR work while capturing. Live transcription can come back after V1 |
| D2 | Audio sources          | **Microphone + system audio**, recorded as **separate tracks**         | Captures every participant. Separate tracks also give basic speaker separation ("Me" vs "Others") for free |
| D3 | Local AI               | **Cloud first, behind provider traits**. Local providers come later as an option (hybrid) | Ships sooner, and no code depends on Addis Scribe directly |
| D4 | Recording control      | Manual start/stop                                                      | No automatic meeting detection in V1 |
| D5 | Language               | Afaan Oromo (`om`) first. The language is a per-meeting field          | Room for more languages later |
| D6 | Internet               | Needed only for transcription and summaries. **Recording never needs it** | Recording and transcription are independent (see §6.4) |
| D7 | Storage                | Local only. Our servers never store audio or transcripts                | Privacy |
| D8 | Accounts               | None                                                                   | No sign-up step |
| D9 | Summaries              | Generated after the meeting, can be regenerated                         | Needs the full transcript |
| D10| Pricing                | Free first. Local AI is the long-term way to keep costs down            | — |
| D11| Stack                  | **Tauri 2 + Rust core + React/TS UI** (see §5a)                         | One small native binary, real-time-safe audio, and a direct path to local AI |

## 4. Out of scope (V1)

Real-time / near-live transcription during recording (deferred, see D1) · team collaboration and shared workspaces · cloud dashboard and accounts · automatic meeting detection · calendar integrations · meeting bots · project and task management · mobile app · Windows and macOS.

---

## 5. Functional requirements

Priority: **P0** = required for V1 release, **P1** = should ship with V1, **P2** = nice to have.

### 5.1 Recording (`FR-REC`)

| ID         | Pri | Requirement |
| ---------- | --- | ----------- |
| FR-REC-01  | P0  | User can start and stop a recording manually from the dashboard and from the system tray. |
| FR-REC-02  | P0  | Capture the default microphone (PipeWire source; PulseAudio fallback). |
| FR-REC-03  | P0  | Capture system output audio (the monitor of the default sink). |
| FR-REC-04  | P0  | Record mic and system at the same time, to **separate, time-aligned tracks**. |
| FR-REC-05  | P0  | Audio is written to disk incrementally, so a crash or power loss loses at most ~5 s. |
| FR-REC-06  | P0  | A recording indicator stays visible the whole time a recording is active, including when the main window is hidden. |
| FR-REC-07  | P1  | User can pick the input and output devices. The choice is remembered. Device IDs are audio-server node names, valid on PipeWire and PulseAudio and stable across reboots. A remembered device that is missing falls back to the system default with a notice; recording never refuses to start over it. |
| FR-REC-08  | P1  | Live level meters for each track, with a warning when a track is silent for more than 30 s. Levels are RMS and peak dBFS over 100 ms of the audio written to the file (floored at −100 dBFS). Silence is RMS below −60 dBFS; a device that stops delivering counts as silent. Paused time is not counted. |
| FR-REC-09  | P1  | Pause and resume. Paused time is cut out of the recording: both tracks continue seamlessly on resume, the recording clock stops while paused, and the pauses are listed so the UI can mark them. Devices stay open and their audio is discarded, so the recording indicator stays visible (shown as paused). |
| FR-REC-10  | P1  | Echo cancellation on the mic track, using the system track as the reference, for users without headphones. |
| FR-REC-11  | P2  | Mute a single track while recording. |

### 5.2 Transcription (`FR-TRN`)

| ID         | Pri | Requirement |
| ---------- | --- | ----------- |
| FR-TRN-01  | P0  | Transcribe Afaan Oromo audio through the configured ASR provider (initially Addis Scribe). |
| FR-TRN-02  | P0  | Transcript segments carry start/end timestamps (ms) and a source track (`mic`/`system`). |
| FR-TRN-03  | —   | **Deferred (post-V1, see D1).** Near-live: transcribed segments appear in the UI during recording, ≤ 20 s behind speech. |
| FR-TRN-04  | P0  | When recording stops, the meeting is chunked and queued for transcription. Any chunk that failed or was never sent (offline, API error) is processed automatically once connectivity returns, or on demand. Segments appear in the UI as chunks complete. |
| FR-TRN-05  | P0  | User can edit transcript text. Edits are saved and never overwritten by re-processing unless the user confirms. |
| FR-TRN-06  | P1  | Clicking a segment plays the audio from that timestamp. |
| FR-TRN-07  | P1  | Re-transcribe a whole meeting, for example after a provider change. |
| FR-TRN-08  | P2  | Rename speaker labels (e.g. "Others" → "Chala"). |

### 5.3 Meeting intelligence (`FR-AI`)

| ID         | Pri | Requirement |
| ---------- | --- | ----------- |
| FR-AI-01   | P0  | Generate a summary after the meeting from the transcript. |
| FR-AI-02   | P0  | Extract key discussion points, decisions, and action items (with owner and due date if mentioned). |
| FR-AI-03   | P0  | Output is in the meeting's language by default, and the user can ask for English. |
| FR-AI-04   | P1  | Regenerate notes. User edits to notes are kept as a separate, user-owned version. |
| FR-AI-05   | P1  | Each decision and action item links to the transcript segment(s) it came from. |
| FR-AI-06   | P2  | Speaker diarization within the `system` track (beyond Me/Others). |

### 5.4 History & organization (`FR-HIS`)

| ID         | Pri | Requirement |
| ---------- | --- | ----------- |
| FR-HIS-01  | P0  | Dashboard lists meetings newest first, showing title, date, duration and processing status. |
| FR-HIS-02  | P0  | Full-text search across titles, transcripts and notes (SQLite FTS5). |
| FR-HIS-03  | P0  | Meeting detail view shows the audio player, transcript and notes. |
| FR-HIS-04  | P0  | Rename and delete meetings. Delete removes audio files too, after confirmation. |
| FR-HIS-05  | P0  | Export notes and transcript as Markdown and plain text. |
| FR-HIS-06  | P1  | Export the transcript as SRT/VTT, and the audio as a mixed `.opus`. |
| FR-HIS-07  | P2  | Import an existing audio file as a meeting. |

### 5.5 Privacy & settings (`FR-PRV`)

| ID         | Pri | Requirement |
| ---------- | --- | ----------- |
| FR-PRV-01  | P0  | No account, login or telemetry by default. |
| FR-PRV-02  | P0  | All data is stored under the user's data directory (§7.1). |
| FR-PRV-03  | P0  | Settings say plainly which data goes to which provider (audio → ASR, transcript text → LLM). |
| FR-PRV-04  | P0  | API keys are stored in the OS keyring (Secret Service), never in plain-text config. Entries are service `jifar`, account = provider ID (e.g. `addis-scribe`). Keys are read per request, so a key added in Settings works without a restart. |
| FR-PRV-05  | P1  | "Local only" mode turns off all network providers. Recording and history still work. |

---

## 5a. Technology stack

| Area | Choice | Why |
| ---- | ------ | --- |
| App shell | **Tauri 2** | ~10–15 MB bundle vs ~150 MB for Electron; low idle RAM; built-in tray, keyring-friendly, `.deb`/AppImage/`.rpm` bundler |
| Core language | **Rust** (stable, edition 2024) + `tokio` | Real-time-safe audio without a GC; one binary with no bundled Python; same language for local AI bindings later |
| UI | **React + TypeScript + Vite** | Mature ecosystem; strict TS |
| Styling | **Tailwind CSS v4 + shadcn/ui** | — |
| UI state/data | **TanStack Query** (backend data), **Zustand** (UI state), **TanStack Router** | Treats Rust commands as a typed async API |
| Rust ↔ TS types | **tauri-specta** | Generated bindings, so the IPC contract can't drift |
| Audio capture | **`pipewire` crate** (native PipeWire streams); **`libpulse-binding`** fallback for PulseAudio-only systems | Direct access to sources and sink monitors, no subprocess |
| Encoding | **libopus** via `opus` + `ogg` crates | Incremental Ogg/Opus writing, no FFmpeg at runtime |
| Resampling / decoding | **`rubato`** / **`symphonia`** | Pure Rust |
| VAD | **Silero VAD** via `ort` (ONNX Runtime), model embedded in the binary | Accurate chunk boundaries; `ort` is reused for local ASR later. ONNX Runtime is loaded at runtime (`load-dynamic`, `ORT_DYLIB_PATH`), so no HTTP crate enters the build and a missing library can only fail chunking, never recording |
| Echo cancellation | **`webrtc-audio-processing`** | Fixes mic bleed (FR-REC-10) |
| Database | **SQLite** via `rusqlite` (bundled, FTS5) + `rusqlite_migration` | Synchronous, simple, fast. Runs on a dedicated DB thread |
| HTTP | `reqwest` (rustls) | — |
| Secrets | `keyring` crate (Secret Service) | FR-PRV-04 |
| Errors / logs | `thiserror` (libs), `anyhow` (app edge), `tracing` | — |
| Tooling | `pnpm`, `cargo`, Vitest, Playwright (UI), `cargo nextest`, `clippy -D warnings`, `rustfmt`, Biome | — |
| Eval tooling only | **Python** (`jiwer`) in `tools/eval/`. **Not shipped** | Best ecosystem for WER/CER |
| Future local AI | **whisper.cpp** (`whisper-rs`) or ONNX ASR via `ort`; **llama.cpp** (`llama-cpp-2`) or Ollama over HTTP | In-process, behind the same provider traits |

**Known tradeoff:** on Linux, Tauri renders with WebKitGTK, not Chromium. Test the UI against WebKitGTK early (M3), especially audio playback, media seeking and CSS features. Never rely on Chromium-only APIs.

---

## 6. Architecture

### 6.1 Components

```
┌──────────────────────────── Tauri app (one process) ────────────────────────────┐
│  WebView (WebKitGTK): React + TS + Tailwind + shadcn/ui                          │
│        │  tauri commands + events (types generated by tauri-specta)              │
│  Rust core (crates/)                                                             │
│   ├─ jifar-audio   capture → ring buffer → AEC/encoder; offline VAD chunker      │
│   ├─ jifar-store   SQLite (dedicated thread), migrations, FTS                    │
│   ├─ jifar-queue   persistent ASR job queue, retries, connectivity               │
│   ├─ jifar-providers  AsrProvider / NotesProvider traits + impls (only network)  │
│   ├─ jifar-core    MeetingService, orchestration, domain types                   │
│   └─ src-tauri     command handlers, tray, window, app state                     │
└──────────────────────────────────────────────────────────────────────────────────┘
```

- **WebView** shows the UI only. It has no direct filesystem or network access. Tauri capabilities allow only Jifar's own commands, and a strict CSP is set. The one exception is audio playback: the read-only `jifar-audio://` URI scheme serves `mixed.opus`, `mic.opus` and `system.opus` from `meetings/<uuid>/` (nothing else), with HTTP range support. The UI fetches the file and plays it from a `blob:` URL (WebKitGTK's GStreamer media pipeline can't load custom schemes directly); CSP allows the scheme only in `connect-src` and `blob:` only in `media-src`.
- **Rust core** owns everything else. Only `jifar-providers` may make network calls.
- **Audio real-time rule:** PipeWire callbacks only copy samples into a lock-free SPSC ring buffer (`rtrb`). They never allocate, lock, do I/O or log. A separate worker thread does encoding, AEC, level metering and file writes. VAD and chunking run offline after stop (§6.3).
- **Crash safety:** a panic in the audio worker is caught. The recording is finalized from what is already on disk, and the meeting moves to `needs_attention`. It never takes down the app silently.

### 6.2 IPC contract (Tauri commands and events)

Commands (all `async`, returning `Result<T, AppError>`):

| Command                | Args                                         | Returns |
| ---------------------- | -------------------------------------------- | ------- |
| `ping`                 | —                                            | `{version, dataDir}` (health check, M0) |
| `devices_list`         | —                                            | `{inputs[], outputs[]}`, each `{id, label, isDefault}`; `id` is an audio-server node name |
| `recording_start`      | `{title?, language, micDevice?, sinkDevice?}` | `{meeting: Meeting, notices[]}` (notices: device fallbacks, a track that couldn't start) |
| `recording_pause` / `recording_resume` | `{meetingId}`                 | `()` |
| `recording_stop`       | `{meetingId}`                                | `{meeting: Meeting, problems[], peaks[]}` (problems → `needs_attention`; peaks: waveform of `mixed.opus`, 0..1) |
| `meetings_list`        | `{cursor?, limit}`                           | `Page<MeetingSummary>` |
| `meeting_get`          | `{meetingId}`                                | `MeetingDetail` |
| `meeting_rename` / `meeting_delete` | `{meetingId, ...}`              | `()` |
| `segment_update`       | `{segmentId, text}`                          | `Segment` |
| `notes_generate`       | `{meetingId, outputLanguage}`                | `Notes` |
| `search`               | `{query, limit}`                             | `SearchHit[]` |
| `export_meeting`       | `{meetingId, format, path}`                  | `()` |
| `settings_get` / `settings_set` | `Settings {micDevice?, sinkDevice?, language}` | `Settings` |
| `provider_key_status`  | `{provider}` (`"addis_scribe"`)              | `{saved, hint?, keyringError?}`. The key itself never crosses IPC; `hint` is its last 4 characters |
| `provider_key_set`     | `{provider, key}`                            | `{saved, hint?, keyringError?}` (stored in the OS keyring, FR-PRV-04) |
| `provider_key_delete`  | `{provider}`                                 | `{saved, hint?, keyringError?}` |
| `provider_key_test`    | `{provider}`                                 | `{status: ok\|no_key\|invalid_key\|no_credits\|unreachable\|failed, message?, balance?, currency?}` (free, via `GET /scribe/usage`) |

Events (Rust → UI):

| Event              | Payload |
| ------------------ | ------- |
| `audio://levels`   | `{meetingId, mic: dBFS?, system: dBFS?, elapsedMs, paused}` (~10 Hz while recording) |
| `audio://silent`   | `{meetingId, track, sinceMs}` (`sinceMs` null when sound is back) |
| `transcript://segments` | `{meetingId, segments[]}` |
| `meeting://status` | `{meetingId, status}` (also for recordings started/stopped from the tray) |
| `queue://status`   | `{pending, failed, online}` |
| `app://error`      | `{code, message, fatal}` |

`AppError` is a serializable enum with stable `code` strings. The UI branches on `code`, never on `message`.

### 6.3 Audio pipeline

- Capture is native PipeWire streams: one for the mic source, one for the default sink's monitor, at 48 kHz mono f32.
- Archive files are `mic.opus` and `system.opus` (Ogg/Opus, ~32 kbps VOIP mode), written page by page.
- For ASR, chunking runs **after recording stops** (D1), not in the capture pipeline. Each track's `.opus` is decoded and cut into chunks of ~15–30 s at Silero VAD silence boundaries (hard cap 30 s). Chunks are resampled to 16 kHz mono 16-bit WAV with `rubato` and saved in `chunks/`. Chunks with no detected speech (< 250 ms) are not written, so they are never sent to ASR, and long silences are trimmed to a ~300 ms pad around speech. Each chunk carries its `start_ms`/`end_ms` on the recording clock. Normal processing, crash recovery and re-transcription (FR-TRN-07) all use this same path.
- AEC (P1) processes the mic track against the system track before encoding.
- Both tracks share one monotonic clock that starts at `recording_start` and stops while paused (FR-REC-09). Every timestamp is in ms of recording time on that clock. Each track's drift against that clock is measured on the encoder worker (lower envelope of wall time minus delivered audio, so delivery jitter is ignored). Above 20 ms it is corrected by rate matching (rubato `Slip`: single frames inserted or dropped behind a crossfade, ≤ 0.5%, no delay, bit-exact pass-through while idle) until the error is below 2 ms. Device stalls (a persistent step > 250 ms) are filled with silence instead. Faster-than-real-time sources (fixtures, offline encoding) are never corrected.
- `mixed.opus` is rendered in Rust on stop, for playback and export.

### 6.4 Transcription queue and recovery

Each chunk is a row in `asr_jobs` with this state machine:

```
pending ──► in_flight ──► done
   ▲            │
   └── failed ◄─┘   (retry with exponential backoff + jitter: 5s → 10 min max)
```

- Jobs are created when the meeting is chunked after recording stops. Nothing is sent to ASR while recording.
- Jobs are persisted. On startup, any job left `in_flight` goes back to `pending`.
- Network loss pauses the queue. Recording continues unaffected.
- The order of jobs doesn't affect results, because segments are sorted by `(start_ms, track)`.
- The meeting is `transcribed` once every job is `done`. Notes are generated after that.

Meeting status: `recording → processing → ready`, with `needs_attention` when any job has failed more than N times or audio finalization reported a problem.

### 6.5 Provider traits

```rust
#[async_trait]
pub trait AsrProvider: Send + Sync {
    fn id(&self) -> &'static str;               // "addis-scribe", later "whisper-local"
    async fn transcribe(&self, chunk: &AudioChunk, language: &LangCode)
        -> Result<Vec<RawSegment>, ProviderError>; // RawSegment { start_ms, end_ms, text, confidence }
}

#[async_trait]
pub trait NotesProvider: Send + Sync {
    fn id(&self) -> &'static str;               // cloud LLM, later "llama-local"/"ollama"
    async fn generate(&self, input: &NotesInput) -> Result<MeetingNotes, ProviderError>;
}
```

`ProviderError` has three classes, and the queue acts on the class alone:

- `Retryable { retry_after? }` (network, timeouts, 429, 5xx, still processing): retry the same request with backoff, honouring `retry_after`.
- `ActionRequired { kind: Auth | Credits }` (no key, key rejected 401/403, balance too low 402): no request can succeed until the user acts. The queue **pauses** for that provider, as when offline, the UI asks the user to fix the key or top up, and the queue resumes afterwards. Attempts are not counted.
- `Permanent` (bad audio, unsupported language, other 4xx): this job fails, and other jobs continue.

*(`ActionRequired` added 2026-11-11, proposed in `providers/addis-scribe.md` §5; flagged for product-owner review.)*

> **OQ-1 (mostly resolved, see [`providers/addis-scribe.md`](./providers/addis-scribe.md)):** Addis Scribe `POST /api/v1/scribe/transcribe` is a batch upload (≤ 1 h / 95 MiB) with `x-api-key` auth, `language=om`, and word timestamps plus segments (`timestamps=word`). Its `request_id` is an idempotency key, set to the `asr_jobs.id`, so retries never double-bill. Price: 3.5 ETB per 1,000 transcript characters. **Still open:** data retention. For providers without timestamps, chunk boundaries are used as segment times.

---

## 7. Data

### 7.1 Storage layout

```
$XDG_DATA_HOME/jifar/            (default ~/.local/share/jifar)
├── jifar.db                     (WAL mode)
└── meetings/<meetingId>/
    ├── mic.opus
    ├── system.opus
    ├── mixed.opus
    └── chunks/<track>-<index>.wav     (deleted after transcription, configurable)
$XDG_CONFIG_HOME/jifar/settings.json  (no secrets)
```

IDs are UUIDv7 (time-sortable).

### 7.2 Schema (SQLite, migrations versioned)

```sql
meetings(id TEXT PK, title TEXT, language TEXT DEFAULT 'om',
         status TEXT, started_at INTEGER, duration_ms INTEGER,
         created_at INTEGER, updated_at INTEGER)

tracks(id TEXT PK, meeting_id TEXT FK, kind TEXT CHECK(kind IN ('mic','system','mixed')),
       path TEXT, duration_ms INTEGER)

asr_jobs(id TEXT PK, meeting_id TEXT FK, track TEXT, chunk_index INTEGER,
         path TEXT, start_ms INTEGER, end_ms INTEGER, provider TEXT,
         state TEXT, attempts INTEGER, next_attempt_at INTEGER, last_error TEXT, updated_at INTEGER)

segments(id TEXT PK, meeting_id TEXT FK, track TEXT, speaker TEXT,
         start_ms INTEGER, end_ms INTEGER, text TEXT, original_text TEXT,
         edited INTEGER DEFAULT 0, confidence REAL, job_id TEXT)

notes(id TEXT PK, meeting_id TEXT FK, version INTEGER, source TEXT CHECK(source IN ('ai','user')),
      provider TEXT, language TEXT, body_json TEXT, created_at INTEGER)

segments_fts / notes_fts  -- FTS5 external-content tables, unicode61 tokenizer
                          -- with tokenchars "'" so Qubee words like ba'e stay whole
```

`MeetingNotes` JSON: `{ summary, keyPoints[], decisions[{text, segmentIds[]}], actionItems[{text, owner?, due?, segmentIds[]}] }`.

---

## 8. Non-functional requirements

| ID       | Requirement |
| -------- | ----------- |
| NFR-01   | Installs on Ubuntu 22.04/24.04 from `.deb` and AppImage. Runtime dependencies: PipeWire (or PulseAudio), WebKitGTK 4.1, and GStreamer `plugins-base` + `plugins-good` (Ogg/Opus playback). ONNX Runtime (`libonnxruntime.so`) ships inside the packages. No Python or FFmpeg needed. |
| NFR-02   | Recording a 2-hour meeting drops no audio (zero ring-buffer overruns). Audio CPU usage stays < 5% on a 4-core laptop. |
| NFR-03   | App ready in < 2 s. Idle RAM < 150 MB. Search returns in < 300 ms across 500 meetings. |
| NFR-04   | Losing the network or a panic in the audio worker never corrupts finished audio. The meeting is recovered to `processing`. |
| NFR-05   | No recording happens without a visible indicator (FR-REC-06). |
| NFR-06   | Network calls go only to the configured providers. A strict CSP stops the WebView from reaching the network. |
| NFR-07   | UI strings are externalized. English and Afaan Oromo UI are P1. |
| NFR-08   | `cargo clippy -D warnings`, `rustfmt`, `tsc --noEmit` and Biome are clean in CI. No `unsafe` outside FFI wrappers, and every `unsafe` block has a `// SAFETY:` comment. |

## 9. Quality & evaluation

Accuracy is measured, not assumed.

- **Eval set:** ≥ 2 hours of real Afaan Oromo meeting audio, with consent, covering several speakers, accents and noise conditions, plus human reference transcripts. It is stored outside the repo.
- **Harness:** `tools/eval/` (Python + `jiwer`). It calls providers through a small Rust CLI (`jifar-eval`), so the eval tests the same code that ships.
- **Metrics:** WER and CER per provider, also split by condition (clean, noisy, overlapping speech). Text normalization rules (punctuation, case, apostrophe handling in Qubee) are documented next to the harness.
- **Gate:** a provider becomes the default only if it beats the current baseline. The V1 target WER is set after the PoC baseline (OQ-2).
- **Summaries:** rubric-based review (faithfulness, coverage of decisions and actions, language quality) on ≥ 10 meetings.

## 10. V1 success criteria

A user can:

1. Install Jifar on Ubuntu.
2. Open it without creating an account.
3. Record their mic and the other participants' audio.
4. Get a reasonably accurate Afaan Oromo transcript (meeting the §9 gate).
5. Generate a useful summary with decisions and action items.
6. Review past meetings without uploading recordings to Jifar servers.
7. Export their notes.

## 11. Milestones

The task breakdown, sequencing and risks are in [`PLAN.md`](./PLAN.md). V1 = M0–M5. M6 comes after V1.

| M  | Goal | Exit criteria |
| -- | ---- | ------------- |
| M0 | Foundations | Workspace, CI (lint/test on Ubuntu 22.04 + 24.04), dependency guard for `jifar-audio`, tauri-specta bindings, strict CSP. |
| M1 | Audio capture PoC (`jifar-audio` crate + CLI) | Records mic + system to separate Opus tracks for 60 min on Ubuntu 24.04 (PipeWire) and 22.04 (PulseAudio fallback). Zero overruns, no drift > 20 ms, survives `kill -9` with playable files. |
| M2 | Transcription PoC | Addis Scribe behind `AsrProvider`. `jifar-eval` CLI + WER/CER baseline. OQ-1 resolved. |
| M3 | Desktop shell | Tauri app: record controls, tray indicator, meeting list and detail views, SQLite, queue and recovery. WebKitGTK playback verified. |
| M4 | Meeting intelligence | Notes generation, transcript editing, search, export. |
| M5 | Packaging | `.deb` + AppImage from the Tauri bundler, first-run device check, auto-update decision. |
| M6 | Local AI | whisper.cpp / ONNX ASR and llama.cpp evaluated with the same harness, and shipped behind the traits if they pass the gate. |

## 12. Open questions

| ID   | Question |
| ---- | -------- |
| OQ-1 | Addis Scribe: ~~streaming support, timestamps, auth, rate limits, cost~~ answered in `providers/addis-scribe.md` (2026-11-11). **Open:** data retention / training use; `ActionRequired` error class. |
| OQ-2 | Target WER/CER for V1, set after the M2 baseline. |
| OQ-3 | Which LLM provider for V1 notes, and how well it handles Afaan Oromo. |
| OQ-4 | Is WebRTC AEC good enough for mic bleed, or do we also need transcript-level dedupe? Measure in M1/M2. |
| OQ-5 | Consent UX: should Jifar remind users to tell other participants they're being recorded? |
