---
name: jifar
description: Development guide for Jifar, a local-first Linux desktop meeting recorder and Afaan Oromo transcription/notes app built with Tauri 2, a Rust core (PipeWire capture, Opus, SQLite, provider traits) and a React/TypeScript/Tailwind/shadcn UI. Use when planning, implementing, reviewing or testing any Jifar feature — recording and audio capture, the transcription queue, Addis Scribe or other ASR/LLM providers, meeting notes, history/search/export, schema/migrations, Tauri commands/events — or when answering scope questions about V1.
---

# Jifar development

The source of truth is `docs/SPEC.md`. Read the relevant section before you change behavior. If a task conflicts with the spec, stop and point out the conflict. Don't quietly diverge. Update the spec in the same change when a decision changes.

## Before starting a task

1. Find the requirement IDs it touches (`FR-REC-*`, `FR-TRN-*`, `FR-AI-*`, `FR-HIS-*`, `FR-PRV-*`, `NFR-*`). Mention them in commits and PR descriptions.
2. Check it against §4 **Out of scope**. Don't build team features, accounts, cloud sync, calendar, bots, auto-detection, mobile, or Windows/macOS support in V1. Push back if asked.
3. Check which milestone (§11) the work belongs to. Prefer finishing the current milestone over starting the next one.

## Stack (spec §5a)

Tauri 2 · Rust core (tokio) · React + TS + Vite · Tailwind v4 + shadcn/ui · TanStack Query/Router + Zustand · tauri-specta · `pipewire` crate (PulseAudio fallback) · libopus/ogg · rubato · Silero VAD via `ort` · `webrtc-audio-processing` · rusqlite (bundled, FTS5) · reqwest · keyring · tracing. Python is used **only** for `tools/eval/`, and it never ships.

Workspace crates: `jifar-audio`, `jifar-store`, `jifar-queue`, `jifar-providers`, `jifar-core`, `src-tauri`. Keep domain logic in the crates. `src-tauri` command handlers stay thin.

## Invariants (never break these)

- **No real-time transcription in V1** (spec D1). Nothing goes to ASR while recording. Chunking (VAD) runs offline after stop, and the transcript appears only after stop. FR-TRN-03 is deferred, so push back if asked to build live transcription.
- **UI follows `docs/DESIGN.md`** (three-column dark layout, violet accent, based on `docs/design/reference-ui.png`).
- **Recording is independent of the network.** `jifar-audio` never depends on `jifar-providers`, `reqwest` or the queue. If the network fails, ASR jobs queue up and recording carries on.
- **Real-time audio callbacks** only push into the lock-free ring buffer. They never allocate, lock, log, do I/O or `await`. All other work happens on the worker thread.
- **Audio is written incrementally** (Ogg/Opus pages). A crash loses ≤ ~5 s. Never buffer a whole meeting in memory.
- **No hidden recording.** Whenever capture is active, a tray indicator is visible.
- **Network boundary:** only `jifar-providers` makes network calls. The WebView has no network access (strict CSP) and calls only Jifar's own Tauri commands. Never widen Tauri capabilities without a spec change.
- **Providers sit behind traits** (`AsrProvider`, `NotesProvider`, spec §6.5). Return `ProviderError::Retryable` or `ProviderError::Permanent` correctly, because the queue relies on the distinction.
- **User edits are sacred.** Re-transcription or note regeneration never overwrites `edited` segments or `source='user'` notes without explicit confirmation.
- **Data stays local.** Store everything under the XDG paths in spec §7.1. Keys go in the keyring. No telemetry.
- **Timestamps** are integer ms relative to the recording's start clock. Every segment has a `track` (`mic` | `system`).

## Conventions

- **Rust:** edition 2024, `thiserror` in crates and `anyhow` only at the app edge. No `unwrap()`/`expect()` outside tests and startup invariants. `unsafe` only in FFI wrappers, each with a `// SAFETY:` comment. `clippy -D warnings` and `rustfmt` must be clean.
- **IPC:** every command and event is typed through tauri-specta. When you add or change one, regenerate the bindings and update the spec §6.2 tables in the same change. Errors are an `AppError` enum with stable `code`s, and the UI branches on `code`.
- **TS:** strict mode, no `any`. Backend data is fetched with TanStack Query wrapping the generated commands. Never call `invoke` with string command names by hand.
- **UI:** the target is WebKitGTK, not Chromium. Avoid Chromium-only APIs and check new CSS and media features on WebKitGTK.
- **DB:** all access goes through `jifar-store` on its dedicated thread. Schema changes go only through numbered migrations. Never edit an applied migration.
- Language codes are ISO 639-1. Afaan Oromo is `om`. Read `meeting.language` and never hardcode it.
- Treat provider output as untrusted. Validate LLM notes against the `MeetingNotes` schema (serde with strict types), and retry or degrade gracefully if validation fails.
- Afaan Oromo text uses Qubee with meaningful apostrophes (`ba'e`) and doubled letters. Never strip or "normalize" them in storage, FTS tokenization (`tokenchars "'"`) or exports.

## Testing expectations

- **`jifar-audio`:** unit-test the capture pipeline (ring buffer → encoder) and the offline chunker (Opus → VAD → chunks) with fixture WAVs through a fake capture source. CI must not need real PipeWire. Keep a manual device check (`cargo run -p jifar-audio --example record`): mic + system, 60 min, `kill -9` recovery, overrun counter = 0.
- **`jifar-queue`:** test the state transitions `pending → in_flight → done/failed`, backoff, restart recovery (stale `in_flight` → `pending`), and offline → online, using a fake `AsrProvider`.
- **Providers:** contract tests against recorded HTTP responses (`wiremock`). Live API tests run only when `JIFAR_LIVE_TESTS=1` is set.
- **UI:** Vitest for components and hooks, with mocked bindings. Playwright smoke tests for the main flows.
- **Accuracy claims** need numbers from `tools/eval/` (spec §9). Never commit eval audio or transcripts, because they are private recordings.

## When asked about scope or priorities

Answer from spec §3 (settled decisions), §4 (out of scope), §5a (stack) and §12 (open questions). If the question is still open (OQ-*), say so and lay out the options. Don't present a guess as a decision.
