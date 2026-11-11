# Jifar — V1 Delivery Plan

| Field   | Value |
| ------- | ----- |
| Status  | Draft v0.1 |
| Date    | 2026-11-07 |
| Source  | [`SPEC.md`](./SPEC.md) v0.2. The spec defines *what* to build. This plan covers *order, tasks and exit gates*. |
| Scope   | V1 = milestones M0–M5. M6 (local AI) comes after V1. |

If this plan and the spec disagree, the spec wins. Fix the conflict in the spec first, then update this plan. Requirement IDs (`FR-*`, `NFR-*`) and open questions (`OQ-*`) refer to the spec.

---

## 1. What "V1" means

**Release bar:** every **P0** is done, and every spec §10 success criterion can be shown on a clean Ubuntu 22.04 and 24.04 machine.

**Target:** the P1 items ship with V1. A P1 item can be cut only by an explicit decision recorded in §9 of this plan, never by quietly dropping it.

**Deferred:** P2 items (FR-REC-11, FR-TRN-08, FR-AI-06, FR-HIS-07) and M6. Pick them up only after V1 ships.

**Never in V1:** anything in spec §4 (teams, accounts, sync, calendar, bots, auto-detection, mobile, Windows/macOS).

## 2. Strategy

1. **Remove the riskiest unknowns first.** The two things that can sink the product are (a) reliable dual-track capture on real Linux machines and (b) acceptable Afaan Oromo ASR accuracy. M1 and M2 prove both before any UI work.
2. **Run M1 and M2 in parallel.** M2 works from fixture WAVs and doesn't need the capture code. They meet in M3.
3. **Build crates, not screens.** Each milestone leaves a tested crate with a CLI or example. `src-tauri` stays a thin layer on top.
4. **Measure, don't assume.** Accuracy (§9 of the spec) and performance (NFR-02/03) have numeric gates checked at milestone exits.

```
M0 Foundations ──┬──► M1 Audio capture PoC ───┐
                 │                            ├──► M3 Desktop shell ──► M4 Intelligence ──► M5 Packaging ──► V1
                 └──► M2 Transcription PoC ───┘                     (OQ-3 needed)        (beta → release)
                      (OQ-1 needed early)
```

**Critical path:** M0 → M1 → M3 → M4 → M5. M2 runs alongside, but it must close OQ-1 before M3 builds the queue on top of the real API shape.

## 3. Milestones

Sizes are relative effort (S ≈ < 1 week, M ≈ 1–2 weeks, L ≈ 2–4 weeks) for one developer working full time. Recalibrate once team size and M1 actuals are known.

### M0 — Foundations (S)

Goal: an empty but complete repo where CI enforces the conventions from day one.

- [x] `git init`, `.gitignore` (target/, node_modules/, eval data, `*.opus`, `*.wav`).
- [ ] License decision (owner).
- [x] Cargo workspace with empty crates: `jifar-audio`, `jifar-store`, `jifar-queue`, `jifar-providers`, `jifar-core`, plus `src-tauri` (Tauri 2 scaffold, React + TS + Vite, Tailwind v4, shadcn/ui, Biome).
- [x] Workspace lints: `unsafe_code` restricted, `clippy::unwrap_used`/`expect_used` denied outside tests, edition 2024.
- [x] CI (GitHub Actions, Ubuntu 22.04 + 24.04): `rustfmt`, `clippy -D warnings`, `cargo nextest`, `tsc --noEmit`, Biome, Vitest. **NFR-08**
- [x] Dependency guard in CI that fails if `jifar-audio` gains a dependency on `jifar-providers`, `jifar-queue` or `reqwest` (`cargo tree` check). This enforces the recording invariant mechanically.
- [x] tauri-specta wired up with a single `ping` command, and a CI check that the generated bindings are up to date.
- [x] Strict CSP and minimal Tauri capabilities in place from the first commit. **NFR-06**
- [x] `tracing` setup and XDG path resolution (`$XDG_DATA_HOME/jifar`, `$XDG_CONFIG_HOME/jifar`). **FR-PRV-02**

**Exit:** a fresh clone builds and passes CI on both Ubuntu versions. `pnpm tauri dev` opens a window that calls `ping` through generated bindings.

### M1 — Audio capture PoC (L) · `jifar-audio`

Goal: record mic and system audio to separate, aligned, crash-safe Opus tracks from a CLI.

- [x] `CaptureSource` trait with a `FakeSource` (WAV fixtures) so the pipeline can be tested in CI without PipeWire.
- [x] Lock-free SPSC ring buffer (`rtrb`) between the RT callback and the worker. Overrun counter exposed. **NFR-02**
- [x] PipeWire backend: default source (mic) and default sink monitor (system), 48 kHz mono f32. **FR-REC-02, FR-REC-03** (`PipeWireSource`, one main loop per stream, F32LE/48 kHz/mono negotiated so PipeWire converts; system uses `stream.capture.sink=true`. Behind the default-on `pipewire` feature.)
- [ ] PulseAudio fallback backend (`libpulse-binding`), selected at runtime. Test it on stock 22.04, which still ships PulseAudio by default. (`PulseSource` behind the default-on `pulse` feature: blocking `pa_simple` record stream, F32LE/48 kHz/mono, system via `@DEFAULT_MONITOR@`. `Backend::detect` asks the Pulse server for its name: PipeWire if it is pipewire-pulse, Pulse otherwise; `JIFAR_AUDIO_BACKEND` / `record --backend` override. Verified through pipewire-pulse on 24.04; **stock 22.04 run still to do.**)
- [x] Shared monotonic clock from start. Drift measurement between tracks, with resampling correction above 20 ms. **FR-REC-04** (Clock, per-track first-sample offsets and drift measurement in `RecordingSession`. `DriftCorrector` on each worker holds the track to the session clock: `Slip` rate matching above 20 ms, silence for stalls > 250 ms. `TrackStats`/`TrackStatus` report the correction; `record` prints the residual mic−system drift. Simulated 500/−800 ppm and a 600 ms stall stay within 20 ms. **Real-device 60-min check still to do** as part of the M1 exit run.)
- [x] Incremental Ogg/Opus writer (~32 kbps VOIP), flushed page by page so a crash loses ≤ ~5 s. **FR-REC-05** (1 s pages, `fdatasync` every 4 s; libopus is built bundled via `opusic-sys`, so it is not a runtime dependency.)
- [x] Offline chunker (runs after stop, **not** in the capture pipeline, per D1): decode `.opus`, Silero VAD (`ort`), 15–30 s chunks at silence boundaries with a hard cap of 30 s, resample to 16 kHz WAV in `chunks/`, and skip chunks with no speech. `cargo run -p jifar-audio --example chunk <file.opus>`. (`chunk_track`: streaming, memory bounded by one chunk, idempotent per track. Silero model embedded; ONNX Runtime 1.31 loaded at runtime via `ORT_DYLIB_PATH`, fetched by `scripts/fetch-onnxruntime.sh`. On a 30-min real speech sample: 87 chunks, 15.2–29.9 s, median 19.4 s, none hard-capped, 129× real time in release.)
- [x] Per-track levels (~10 Hz) and silence detection (> 30 s). **FR-REC-08** (`LevelMeter` on the worker after drift correction: `TrackStatus::level` (RMS/peak dBFS per 100 ms), `silent_ms`, `silence_warning()`, `SessionStatus::silent_tracks()`. Metering the corrected stream makes a dead device read as silent. The `audio://levels` event itself is M3.)
- [x] Pause/resume that keeps the timeline consistent. **FR-REC-09** (`RecordingSession::pause/resume`: paused audio is discarded in `RtSink::push` (one atomic check), the clock and drift measurement run on recording time, `SessionSummary::pauses` lists the cuts. Per-track cut differences are absorbed by drift correction, so pauses never accumulate misalignment or count as stalls. `record --pause AT:LEN`.)
- [x] Device enumeration and selection. **FR-REC-07** (`Backend::list_devices` over the PulseAudio protocol (real PulseAudio and pipewire-pulse); IDs are node names valid for both backends. `Backend::open(target, device)`: PipeWire `target.object`, Pulse source / `<sink>.monitor`. `resolve_device` falls back to the default for missing or wrong-kind devices. `record --list-devices / --mic / --output`. Verified on 24.04: both backends link to a chosen non-default sink. Remembering the choice is settings work in M3. Pure PipeWire without pipewire-pulse records defaults but can't list.)
- [x] Worker panic handling: catch the panic, finalize what's on disk, report `needs_attention`. (`run_worker` wraps the loop in `catch_unwind`; on a panic or write error it publishes the fault (`TrackStatus::fault`) and still finalizes the Ogg stream, itself guarded. `SessionSummary::needs_attention()`/`problems()` cover failed starts, worker faults and overruns; M3 maps them to the meeting status. The other track is unaffected. Release profiles must keep `panic = "unwind"`, see M5.)
- [x] `cargo run -p jifar-audio --example record` CLI. (`<dir> [--duration SECS] [--mic-only|--system-only]`, prints overruns and drift every 5 s.)
- [ ] **Spike:** `webrtc-audio-processing` AEC on mic vs. system. Measure the bleed reduction to answer **OQ-4**. Full integration can land in M3/M4. **FR-REC-10**

**Exit (spec §11):** 60 min mic + system on Ubuntu 24.04 (PipeWire) and 22.04 (PulseAudio). Zero overruns. Drift ≤ 20 ms. Files are playable after `kill -9`. Audio CPU < 5% on a 4-core laptop. A short written result for the OQ-4 spike.

### M2 — Transcription PoC (M) · `jifar-providers`, `tools/eval`

Goal: know how good Addis Scribe is on real Afaan Oromo meetings, behind the trait the app will use.

- [ ] **Week 1: close OQ-1.** (Docs read 2026-11-11: `docs/providers/addis-scribe.md`. Left: data retention answer from Addis AI, live-API verification with a key, `ActionRequired` decision.) Get API access and document streaming vs. batch, auth, limits, timestamps, cost per hour and data retention in `docs/providers/addis-scribe.md`. Update spec §6.5/§12.
- [ ] `AsrProvider`, `NotesProvider`, `ProviderError { Retryable, Permanent }`, plus domain types (`AudioChunk`, `RawSegment`, `LangCode`).
- [ ] Addis Scribe implementation with correct error classification (network/5xx/429 → `Retryable`; auth/4xx → `Permanent`). If the API returns no timestamps, fall back to chunk boundaries. **FR-TRN-01, FR-TRN-02**
- [ ] `wiremock` contract tests built from recorded responses. Live tests gated behind `JIFAR_LIVE_TESTS=1`.
- [ ] API key stored in and read from the keyring (`keyring` crate). **FR-PRV-04**
- [ ] `jifar-eval` Rust CLI: runs a provider over a directory of audio and writes hypotheses.
- [ ] `tools/eval/` (Python + `jiwer`): WER/CER overall and by condition. Document the Qubee normalization rules (apostrophes, doubled letters, punctuation, case).
- [ ] Collect the eval set: ≥ 2 h of consented Afaan Oromo meeting audio plus reference transcripts, stored **outside the repo**. *(Long lead time, so start during M0.)*
- [ ] Record the baseline and propose the V1 target to close **OQ-2**.

**Exit:** OQ-1 closed, baseline WER/CER published (numbers only, no audio or transcripts committed), OQ-2 target agreed. **Go/no-go:** if the baseline is unusable, stop and revisit D3 (pull local/alternative ASR forward) before M3.

### M3 — Desktop shell (L) · `jifar-store`, `jifar-queue`, `jifar-core`, `src-tauri`, UI

Goal: the end-to-end loop works in the real app. Record → stop → chunk + transcribe → meeting reviewable, including after a restart. **No transcription while recording** (spec D1).

Backend:
- [ ] `jifar-store`: dedicated DB thread, WAL, `rusqlite_migration`, migration 001 = spec §7.2 schema, FTS5 tables with `tokenchars "'"`. UUIDv7 IDs.
- [ ] `jifar-queue`: persistent `asr_jobs` state machine, exponential backoff + jitter (5 s → 10 min), restart recovery (`in_flight` → `pending`), connectivity detection that pauses and resumes the queue, manual "retry now". **FR-TRN-04**
- [ ] `jifar-core::MeetingService`: start/stop/pause orchestration. On stop: render `mixed.opus`, run the offline chunker, create jobs, then jobs → segments. Meeting status `recording → processing → ready / needs_attention`. A crash mid-recording recovers by chunking whatever is on disk.
- [ ] Tauri commands and events from spec §6.2 (recording, meetings, devices, settings), all typed via tauri-specta. `AppError` with stable codes. (Done for recording, devices and settings: `RecordingService` in `jifar-core`, all four events. Meetings commands wait for `jifar-store`.)
- [x] System tray with start/stop and a recording indicator visible whenever capture is active. **FR-REC-01, FR-REC-06, NFR-05** (Always-visible tray; red record dot + elapsed time while capturing, "microphone still open" while paused. Exiting the app finalizes an active recording.)

UI:
- [ ] App layout, TanStack Router, and a query layer over the generated bindings.
- [ ] Dashboard: meeting list (newest first, status badges) and a record button. **FR-HIS-01, FR-REC-01**
- [ ] Recording view: timer, waveform/level meters, silence warning. The transcript area shows a "Transcript will appear after you stop" state, with no live segments. **FR-REC-08**
- [ ] Processing view: progress (chunks done / total), segments filling in as jobs complete, and offline/failed state with "Retry". **FR-TRN-04**
- [ ] Meeting detail: audio player, transcript (Me/Others by track), click a segment to seek. **FR-HIS-03, FR-TRN-06**
- [ ] Rename and delete (delete removes files after confirmation). **FR-HIS-04**
- [ ] Settings: devices, provider keys, a plain-language data-flow explanation. **FR-PRV-03, FR-REC-07**
- [ ] **WebKitGTK playback check early in M3:** Ogg/Opus playback and seeking. See risk R3. (`mixed.opus` rendered on stop with a waveform; player in the saved-recording view. Waiting on a run in the real app.)

**Exit:** record a 30-min call, stop, and get a full transcript with no manual steps. Stop while offline and see the meeting wait in `processing`, then the queue drain once the network is back. `kill -9` mid-recording, relaunch, and get the meeting recovered. Playback and seek work on 22.04 and 24.04. Startup < 2 s, idle RAM < 150 MB. **NFR-03, NFR-04**

### M4 — Meeting intelligence (L) · `jifar-providers`, `jifar-core`, UI

Goal: turn transcripts into useful notes, and make history searchable and exportable.

- [ ] **Close OQ-3 first:** shortlist 2–3 LLMs, run them on ≥ 10 eval meetings with the §9 rubric, pick one.
- [ ] `NotesProvider` implementation. Prompt produces `MeetingNotes` JSON. Strict serde validation, then retry, then degrade gracefully. Output in the meeting language, or English on request. **FR-AI-01, FR-AI-02, FR-AI-03**
- [ ] Segment references in decisions and action items, rendered as links that jump to the transcript. **FR-AI-05**
- [ ] Notes versioning: regenerating creates a new `ai` version, and user edits create a `user` version that is never overwritten. **FR-AI-04**
- [ ] Transcript editing (`edited` flag, `original_text` kept). Re-processing skips edited segments unless the user confirms. **FR-TRN-05**
- [ ] Re-transcribe a whole meeting. **FR-TRN-07**
- [ ] Search across titles, transcripts and notes, with snippets. < 300 ms over 500 meetings, checked with a generated benchmark DB. **FR-HIS-02, NFR-03**
- [ ] Export Markdown and plain text (P0), plus SRT/VTT and mixed `.opus` (P1). Exports preserve Qubee apostrophes. **FR-HIS-05, FR-HIS-06**
- [ ] "Local only" mode that disables all providers, while recording and history keep working. **FR-PRV-05**
- [ ] Finish AEC integration if the OQ-4 spike says so. **FR-REC-10**
- [ ] Externalize UI strings. English complete, Afaan Oromo translation started. **NFR-07**

**Exit:** spec §10 criteria 3–7 can be shown end to end. The summary rubric passes on ≥ 10 meetings. WER meets the OQ-2 target.

### M5 — Packaging & release (M)

- [ ] Keep `panic = "unwind"` in release profiles: audio worker panic recovery relies on it.
- [ ] `.deb` and AppImage from the Tauri bundler. Declare runtime dependencies (PipeWire/PulseAudio, WebKitGTK 4.1, GStreamer plugins needed for playback). **NFR-01**
- [ ] Clean-VM install tests on 22.04 and 24.04 (scripted where possible).
- [ ] First-run flow: device check (records 3 s from each track and shows levels), provider key setup, privacy explanation, consent reminder if OQ-5 decides one.
- [ ] Auto-update decision (Tauri updater vs. manual vs. distro repo), recorded in the spec.
- [ ] Afaan Oromo UI translation complete. **NFR-07**
- [ ] Closed beta with 5–10 real users for 2 weeks, with a bug bash and a fix window.
- [ ] Release checklist: every P0 verified, P1 status recorded, `README` install docs, changelog.

**Exit = V1 release:** spec §10 success criteria 1–7 pass on clean installs, with no open P0 bugs.

### After V1 — M6 Local AI

whisper.cpp / ONNX ASR and llama.cpp evaluated with the same `jifar-eval` harness. They ship behind the existing traits only if they pass the §9 gate. Not scheduled here.

## 4. Requirement → milestone map

| Milestone | Requirements |
| --------- | ------------ |
| M0 | NFR-06, NFR-08, FR-PRV-02 |
| M1 | FR-REC-02, 03, 04, 05, 07, 08, 09 · NFR-02 · (FR-REC-10 spike) |
| M2 | FR-TRN-01, 02 · FR-PRV-04 · spec §9 baseline |
| M3 | FR-REC-01, 06 · FR-TRN-04, 06 · FR-HIS-01, 03, 04 · FR-PRV-01, 03 · NFR-03, 04, 05 |
| M4 | FR-TRN-05, 07 · FR-AI-01, 02, 03, 04, 05 · FR-HIS-02, 05, 06 · FR-PRV-05 · FR-REC-10 · NFR-07 (start) |
| M5 | NFR-01 · NFR-07 (finish) · spec §10 |
| Deferred | FR-TRN-03 (near-live transcription, per D1) · FR-REC-11, FR-TRN-08, FR-AI-06, FR-HIS-07 (P2) |

## 5. Open questions: when each must close

| OQ | Blocks | Close by | Owner |
| -- | ------ | -------- | ----- |
| OQ-1 Addis Scribe API | queue design, chunk sizing | M2 week 1 | TBD |
| OQ-2 WER target | V1 accuracy gate | M2 exit | TBD |
| OQ-3 LLM for notes | M4 notes work | M4 start (spike during M3) | TBD |
| OQ-4 AEC vs. dedupe | FR-REC-10 design | M1 exit (spike) | TBD |
| OQ-5 Consent UX | first-run flow | M5 start | TBD |
| New: auto-update | M5 | M5 | TBD |

## 6. Spec gaps found while planning

These need a spec decision. Under the project rules they can't just be added in code.

1. **Missing IPC commands.** Spec §6.2 has no command for: retrying failed jobs on demand (FR-TRN-04), re-transcribing a meeting (FR-TRN-07), saving user-edited notes (FR-AI-04), cancelling a recording, or getting queue status on demand. Proposal: `queue_retry {meetingId?}`, `meeting_retranscribe {meetingId, confirmOverwriteEdits}`, `notes_save {meetingId, body}`, `queue_status`.
2. ~~**Audio playback path into the WebView.**~~ Resolved 2026-11-11: option (b), `jifar-audio://` (spec §6.1). The WebView has no filesystem access, but FR-TRN-06 / FR-HIS-03 need it to play `mixed.opus`. Options: (a) Tauri asset protocol scoped to `$XDG_DATA_HOME/jifar/meetings/**`, or (b) a custom `jifar-audio://` URI scheme handler that serves byte ranges. Either one widens capabilities, so it needs a spec change. Recommendation: (b), because it keeps the scope explicit and supports range requests for seeking.
3. ~~**GStreamer runtime dependency.**~~ Resolved 2026-11-11: listed in NFR-01. WebKitGTK plays media through GStreamer. Ogg/Opus playback needs `gstreamer1.0-plugins-base/good` installed, so NFR-01 should list them.
4. **Chunk retention default.** Spec §7.1 says chunks are deleted after transcription and that this is "configurable". Name the setting and its default, and decide whether re-transcription (FR-TRN-07) regenerates chunks from the Opus tracks. It must, if chunks are deleted.
5. ~~Silent chunks.~~ Resolved in spec v0.3 §6.3: chunks with no speech are not sent to ASR.
6. **Keywords.** The design reference has a Keywords panel, but `MeetingNotes` (spec §7.2) has no `keywords` field. Proposal: add `keywords[]` to `MeetingNotes` as a new P1 requirement, FR-AI-07.

## 7. Risks

| # | Risk | Impact | Mitigation |
| - | ---- | ------ | ---------- |
| R1 | Addis Scribe accuracy too low, or the API unsuitable (no timestamps, strict limits, unclear retention) | Product fails criterion 4 | Close OQ-1 in M2 week 1. Go/no-go at M2 exit. Traits allow swapping providers. Keep M6 options warm. |
| R2 | PipeWire/PulseAudio differences across distros and devices (Bluetooth, USB headsets, sink switching mid-call) | Lost or garbled audio | Fake source for CI, plus a manual device matrix in M1. Handle default-device changes explicitly. |
| R3 | WebKitGTK media quirks (Opus playback, seeking, missing GStreamer plugins) | Detail view unusable on some machines | Test in the first week of M3. Fallback: serve WAV/PCM ranges through the custom scheme. |
| R4 | Mic bleed (no headphones) duplicating text across tracks | Messy transcripts | OQ-4 spike in M1. AEC (FR-REC-10), with transcript dedupe as a fallback. |
| R5 | LLM quality in Afaan Oromo | Weak notes | OQ-3 rubric eval before committing. Option to generate in English (FR-AI-03). |
| R6 | Eval data collection (consent, transcription effort) takes longer than expected | M2 slips, no accuracy gate | Start in M0. Begin with a smaller set and grow it to 2 h. |
| R7 | Native dependency build complexity (libopus, ONNX Runtime, webrtc-audio-processing) | CI and packaging pain | Pin versions, vendor or static-link where possible, and verify the AppImage on a clean VM early in M3 rather than waiting for M5. |
| R8 | Recording-consent legal and ethical exposure | User and legal risk | Decide OQ-5 before beta. A visible indicator is already mandatory. |

## 8. Definition of done (every task)

- Requirement IDs referenced in the commit or PR.
- Tests at the level the jifar skill requires (fake capture for audio, fake provider for queue, wiremock for providers, Vitest/Playwright for UI).
- `clippy -D warnings`, `rustfmt`, `tsc --noEmit` and Biome clean. Bindings regenerated if IPC changed.
- Spec updated in the same change if behavior, IPC (§6.2) or schema (§7.2) changed. Schema changes only through new migrations.
- No invariant broken (network independence of recording, RT-safe callbacks, user edits preserved, no hidden recording, local-only data).

## 9. Decision log

Record plan-level decisions here (P1 cuts, re-sequencing, go/no-go outcomes). Product decisions go in spec §3.

| Date | Decision | Why |
| ---- | -------- | --- |
| 2026-11-07 | V1 = M0–M5. M6 local AI moves after V1. | D3: cloud first. Keeps V1 focused. |
| 2026-11-07 | Added M0 Foundations ahead of M1. | CI and invariant guards should exist before the first feature code. |
| 2026-11-07 | M1 and M2 run in parallel. | Independent crates. M2 works on fixtures. |
| 2026-11-07 | **No real-time transcription in V1.** Transcribe after stop only. FR-TRN-03 deferred (spec D1, v0.3). | Product owner decision. Also takes VAD/ASR work out of the live capture path. |
| 2026-11-07 | Chunking runs offline after stop, not during capture. | One code path for normal processing, crash recovery and re-transcription. Lower CPU and risk while recording. |
| 2026-11-07 | UI follows the reference design in [`DESIGN.md`](./DESIGN.md). | Product owner choice. |
| 2026-11-07 | **UI shell built first**, ahead of M1, against a mock backend (`src/lib/mock-backend.ts`) that mirrors spec §6.2. | Product owner wants to validate the design early. The mock is replaced by tauri-specta bindings in M3. No Rust toolchain needed yet. |
| 2026-11-11 | M3's recording slice (core service, commands/events, tray, UI wiring, playback) done before `jifar-store`. | Product owner wants to test recording through the real UI. Same work, different order. |
| 2026-11-11 | Audio reaches the WebView only through the read-only `jifar-audio://` scheme. | Product owner asked for playback now; scheme keeps the scope explicit (PLAN §6.2). |
| 2026-11-11 | ONNX Runtime is loaded dynamically (`ort` `load-dynamic`), not downloaded at build time. | ort's build-time download pulls `ureq` into jifar-audio, which `check-deps.sh` forbids. Dynamic loading keeps the guard strict, and a missing library can only affect chunking. M5 packaging must bundle `libonnxruntime.so` (R7). |
