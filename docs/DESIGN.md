# Jifar — UI Design

| Field  | Value |
| ------ | ----- |
| Status | Draft v0.1 |
| Date   | 2026-11-07 |
| Reference | [`design/reference-ui.png`](./design/reference-ui.png) (the chosen visual direction) |

The reference image is a **style and layout reference**, not a pixel spec. Its macOS window buttons, "Real-time audio" selector and placeholder text don't apply to Jifar. This document records what we take from it and how it maps to Jifar's V1 features ([`SPEC.md`](./SPEC.md)).

> **V1 has no real-time transcription** (spec D1). The transcript column is empty while recording and fills in only after the user presses stop.

## 1. Visual language

- **Dark theme first.** Near-black window, slightly lighter panels, and rounded "cards" for transcript segments. Light theme is optional later.
- **Single accent: violet/purple** for the primary record button, the waveform, the AI sparkle icon and focus rings. **Red** only for stop and destructive actions.
- **Calm, low-contrast chrome.** Thin dividers, muted secondary text, no heavy borders.
- **Generous spacing** and large readable transcript text (Afaan Oromo segments can be long).
- Icons: outline style (Lucide, which ships with shadcn/ui).

### Tokens (starting values, tune in implementation)

| Token | Value | Use |
| ----- | ----- | --- |
| `--bg` | `#0f0f14` | Window background |
| `--panel` | `#16161d` | Sidebar, right panel |
| `--card` | `#1e1e27` | Transcript segment cards |
| `--border` | `#2a2a35` | Dividers, chip outlines |
| `--text` | `#ececf1` | Primary text |
| `--text-muted` | `#9a9aab` | Labels, timestamps |
| `--accent` | `#7c5cff` | Record button, waveform, highlights |
| `--accent-glow` | `#7c5cff40` | Ring around the active record button |
| `--danger` | `#ef4444` | Stop button, delete |
| Radius | 10–12 px cards, full for round buttons | |
| Font | Inter (bundled, no network fetch), 15–16 px transcript body | |

Define these as Tailwind v4 theme variables / shadcn CSS variables. Fonts are bundled locally because the WebView has no network access (NFR-06).

## 2. Layout: three columns

```
┌──────────────┬──────────────────────────────────┬────────────────────────┐
│ Sidebar      │ Header: meeting title    [⚙]     │                        │
│  Jifar logo  ├──────────────────────────────────┤  ✦ Summary             │
│              │                                  │                        │
│ [⏸] (●) [■]  │  Me · 00:01:12                   │  Key points            │
│  device ▾    │  ┌────────────────────────────┐  │   • …                  │
│              │  │ segment text…              │  │                        │
│ ∿∿ waveform  │  └────────────────────────────┘  │  Decisions             │
│   00:12:34   │  Others · 00:01:20               │   • … ↗ (jump to seg)  │
│              │  ┌────────────────────────────┐  │                        │
│ Search…      │  │ segment text…              │  │  Action items          │
│ Meetings     │  └────────────────────────────┘  │   ☐ … — owner, due     │
│  • Today …   │                                  │                        │
│  • Yesterday │  (audio player bar, pinned)      │  Keywords  [chip][chip]│
│ ──────────── │                                  │                        │
│ ⚙ Settings   │                                  │  [Regenerate] [Export] │
└──────────────┴──────────────────────────────────┴────────────────────────┘
```

| Column | Reference element | Jifar content | Requirements |
| ------ | ----------------- | ------------- | ------------ |
| **Left sidebar** | Logo, 3 round buttons, "Real-time audio" dropdown, waveform, settings links | Logo. **Pause/resume · Record · Stop** buttons. Input device picker (replaces the "Real-time audio" dropdown). Live waveform + elapsed time while recording. **Search + meeting history list** (replaces the duplicated "Settings" links). Settings at the bottom. | FR-REC-01, 07, 08, 09 · FR-HIS-01, 02 |
| **Center** | Transcript cards with speaker labels | Transcript of the selected meeting. Label = `Me` (mic track) / `Others` (system track), plus timestamp. Click a card to seek audio; edit inline. Audio player pinned at the bottom. | FR-TRN-02, 05, 06 · FR-HIS-03 |
| **Right panel** | "AI Summary" with bullets, Keywords chips | **Summary**, **Key points**, **Decisions**, **Action items** (owner/due), **Keywords**. Decisions and actions link to their source segments. Regenerate, language toggle (meeting language / English), export. | FR-AI-01 – 05 · FR-HIS-05 |

Linux window chrome: use native GTK decorations for V1. A custom title bar can come later.

## 3. States

Post-meeting transcription only (D1), so the center and right columns depend on the meeting status:

| State | Sidebar | Center | Right panel |
| ----- | ------- | ------ | ----------- |
| **Idle / empty** (first run, no meetings) | Big record button, device picker | Empty state: "Press record to capture your meeting" | Hidden or a privacy note (FR-PRV-03) |
| **Recording** | Record button glowing, red stop active, waveform + timer, silence warning | **No transcript.** Calm placeholder: "Recording… the transcript will appear after you stop." Per-track level meters (Me / Others). | Hidden |
| **Processing** (after stop) | Normal | Progress "Transcribing 7 / 24 chunks". Segments fill in as chunks finish. Offline: "Waiting for connection" + Retry | Skeleton, then "Generate notes" once transcribed |
| **Ready** | Normal | Full transcript, editable | Notes |
| **Needs attention** | Badge in the history list | Banner explaining the failure + Retry / Re-transcribe | As available |

The tray icon shows a recording dot whenever capture is active, even when the window is hidden (FR-REC-06).

## 4. Open design items

- **Keywords** need a spec addition (`keywords[]` in `MeetingNotes`, proposed FR-AI-07). See PLAN §6.
- Afaan Oromo UI strings (NFR-07): check that labels like "Action items" still fit in the sidebar and chips once translated.
- Responsive behavior: below ~1100 px, collapse the right panel into a tab next to the transcript.
- Verify the blur/glow effects and fonts render correctly on WebKitGTK (M3).
