# Addis Scribe (Addis AI speech-to-text): provider notes

| Field  | Value |
| ------ | ----- |
| Status | OQ-1 findings, from the public docs (read 2026-11-11). Not yet verified against the live API. |
| Source | https://docs.addisassistant.com/docs/capabilities/speech-to-text, `/platform/limits`, `/platform/errors`, `/get-started/quickstart` |
| Closes | Most of OQ-1 (spec §12). Open items listed at the end. |

## 1. Which API

Addis AI has two speech-to-text APIs. Jifar uses **Scribe**.

| API | Endpoint | Fit |
| --- | -------- | --- |
| Legacy | `POST /api/v2/stt` | No. 60 s / 10 MB max, text only, no timestamps. |
| **Scribe** | `POST /api/v1/scribe/transcribe` | Yes. Up to 1 h / 95 MiB per upload, word timestamps, segments, speaker labels, idempotent request IDs, result recovery. |

Base URL `https://api.addisassistant.com`. Auth header `x-api-key: sk_...` (keys from https://addisassistant.com/apikeys). There are official SDKs for Node and Python only, so Jifar calls the HTTP API from Rust (`reqwest`, multipart) in `jifar-providers`.

## 2. The request Jifar sends

One request per VAD chunk (spec §6.3):

```
POST /api/v1/scribe/transcribe?backend=turbo&language=om&timestamps=word&request_id=<asr_jobs.id>
x-api-key: <key from keyring>
Content-Type: multipart/form-data
  audio = <chunks/mic-0003.wav>   (16 kHz mono 16-bit WAV, ≤ 30 s)
```

- `language=om` is Afaan Oromo (model `addis-scribe-oromo-streaming`). The value comes from `meeting.language`, never hardcoded. **Leaving out `language` means Amharic**, so the provider must always send it.
- `backend=turbo` is the only backend for `om`; `standard` is Amharic only and returns 422 for anything else.
- `timestamps=word` returns `data.words[]` and `data.segments[]` with `start`/`end` in seconds from the start of the uploaded file.

Response (abridged):

```json
{ "data": {
    "text": "…",
    "segments": [ { "text": "…", "start": 0.6, "end": 3.1 } ],
    "words":    [ { "text": "…", "start": 0.6, "end": 0.9 } ],
    "seconds": 29.4, "compute_ms": 812, "usage": { … }
} }
```

Mapping to `RawSegment { start_ms, end_ms, text, confidence }`: each Scribe segment becomes one `RawSegment`, with `start_ms = chunk.start_ms + round(start * 1000)`, and likewise `end_ms`. Scribe segments are caption cues (they break at 0.5 s pauses, 2×42 characters or 7 s), which suits the transcript view. `confidence` isn't returned by Scribe, so it's `None`. Text is stored exactly as returned (Qubee apostrophes and doubled letters intact).

**The spec's fallback for "no timestamps" (§6.5) isn't needed for Scribe**, but stays for other providers.

## 3. Why keep 15–30 s chunks when Scribe takes 1 h files

Scribe could transcribe a whole track in one upload. Jifar still sends VAD chunks:

- **Resilience.** A network drop costs one 30 s chunk, not an hour. That matters on unstable connections, which is the D6 scenario.
- **Progress.** Segments appear chunk by chunk (FR-TRN-04).
- **Cost.** Billing is per output character, not per second, so chunking costs nothing extra. Silent chunks are never sent at all.
- **Provider independence.** Other ASR providers (and M6 local models) need short chunks.

Trade-off: a chunk boundary can cut context. The chunker cuts at pauses, which limits this. If the eval (M2) shows boundary errors, try 60 s chunks (still within limits).

## 4. Idempotency and retries: a very good fit for the queue

Scribe's `request_id` is an idempotency key:

- A replayed `request_id` returns the original result **with no second charge**.
- The same `request_id` with different audio or settings returns 409 `IDEMPOTENCY_CONFLICT`.
- Results stay recoverable for 24 h via `GET /api/v1/scribe/requests/{request_id}`.

So: **`request_id` = the `asr_jobs.id` (UUIDv7)**. A retry after a timeout or crash can never double-bill, and after a restart a job left `in_flight` can be recovered instead of re-sent. Re-transcription (FR-TRN-07) creates new jobs, so it gets new IDs.

Slow requests: past about 75 s, `/transcribe` answers **202** with `poll_url` and `Retry-After`, and no transcript. Jifar doesn't poll. The provider returns `Retryable { retry_after }` and the **queue** retries later with the same `request_id`, getting either the finished result (not billed again) or 409 `IN_PROGRESS` (retryable again). This is rare for 30 s chunks, and it never ties up a transcription slot. The cost is re-uploading a ~1 MB chunk. (Decided 2026-11-11.)

## 5. Error classification (`ProviderError`, spec §6.5)

| Response | Class | Notes |
| -------- | ----- | ----- |
| Network error, timeout | Retryable | Same `request_id` on retry |
| 202 processing, 409 `IN_PROGRESS` | Retryable | Queue retries with the same `request_id`; honour `Retry-After` |
| 409 `REQUEST_INTERRUPTED` | Retryable | Backend stayed busy |
| 429 (`BUSY`, capacity, account protection) | Retryable | Exponential backoff; honour `Retry-After` if present. No `x-ratelimit-*` headers |
| 500, 503 | Retryable | |
| 401, 403 (`invalid_api_key`) | **Needs user action** | Every job fails until the key is fixed |
| 402 `INSUFFICIENT_CREDITS` | **Needs user action** | Every job fails until the wallet is topped up |
| 400, 413, 422 (`INVALID_AUDIO`, `UNSUPPORTED_LANGUAGE`, `INVALID_REQUEST`) | Permanent | This chunk can't succeed as is |
| 409 `IDEMPOTENCY_CONFLICT` | Permanent | A bug on our side; log loudly |

Error body: `{"status":"error","error":{"code":"…","message":"…","param":"…"}}`. Every response carries `X-Request-Id`; Jifar can send its own (≤ 128 chars of letters, digits, `.`, `_`, `:`, `-`), so use the job ID there too for support requests.

**Proposed spec change:** 401/403/402 fit neither class. Retrying burns attempts without hope, and "permanent" would fail every chunk of the meeting. Proposal: add `ProviderError::ActionRequired { kind: Auth | Credits }`. The queue then **pauses** (like offline) and the UI asks the user to fix the key or top up, then resumes. Needs a decision before the queue is built.

## 6. Limits and cost

- Upload: 95 MiB and 1 h per request (a 30 s chunk is about 1 MB). Long-audio jobs up to 3 h exist but aren't needed.
- No fixed request quotas; pay-as-you-go wallet in ETB. 429 when at capacity or after unusual traffic.
- **Price: 3.5 ETB per 1,000 transcript characters** (UTF-16 units, final text only), as of 2026-10-04. Turbo and Standard cost the same.
- Rough estimate: an hour of continuous conversation is about 7,800 words, roughly 65,000 characters of Afaan Oromo, so **about 230 ETB per hour of speech**. Silence costs nothing. To be replaced by measured numbers from the M2 eval.
- `GET /api/v1/scribe/usage` returns the current rate and wallet balance, which could be shown in Settings.

## 7. Not used in V1 (but worth knowing)

- **Live WebSocket transcription** (`/scribe/sessions`, 15 min max, text only). V1 transcribes after stop only (spec D1).
- **`speakers=true`** (Turbo, up to 8 speakers, `om` supported). It could label speakers within the `system` track, which is FR-AI-06 (P2). Free to enable later, since the cost is per character.
- **`format=srt|vtt`.** Jifar renders its own exports from segments (FR-HIS-06).

## 8. Notes (OQ-3) candidate

Addis AI also has an LLM, Addis-፩-አሌፍ: `POST /api/v1/chat_generate`, 128k context, 4,096-token max output, `target_language: "om"`, with Afaan Oromo "fully supported". It's a natural OQ-3 candidate: same key, same vendor, Oromo-first. Caveats: the docs list "structured output" as a use case but show **no JSON mode or schema parameter**, so `MeetingNotes` validation and retry (spec FR-AI) would carry the load. 4,096 output tokens is enough for notes. Pricing isn't on that page.

### Implementation (2026-11-11): `AddisNotes` (`jifar-providers/src/addis_notes.rs`)

- `POST /api/v1/chat_generate` with the documented REST body: `prompt`, `target_language` (the output language), `generation_config: {temperature: 0.2, maxOutputTokens: 4096}`. The same Addis AI key as Scribe (keyring `jifar/addis-ai`).
- The transcript is sent as `[s12] Others 03:15: …` lines. Short IDs are mapped back to real segment IDs afterwards, and invented ones are dropped.
- The answer is read from `response_text` or `choices[0].message.content` (the docs show both). `finish_reason: "content_filter"` counts as a permanent failure, never as empty notes.
- Without a JSON mode: strict `MeetingNotes::parse`, then one retry quoting the exact error. If that also fails, a prose reply becomes a summary-only result; broken JSON fails.
- Errors are classified as for Scribe (401/403/402 → `ActionRequired`, 429/5xx → `Retryable`).
- To verify with a key: the real response shape, JSON reliability on Oromo meetings, and pricing (not on the docs page). CLI: `scribe notes <meeting-dir> [--out en]`.

## 9. Still open (OQ-1 remainder)

1. **Data retention and privacy.** The docs say nothing about how long audio is kept or whether it's used for training. Results are kept at least 24 h for recovery. Needed for FR-PRV-03 and D7. Ask support@addisassistant.com.
2. **Real accuracy on Afaan Oromo meetings.** The M2 eval (spec §9) measures this.
3. **Verify against the live API** with a test key: the response shape, `usage` fields, and real error bodies, recorded as `wiremock` fixtures.
4. **`ActionRequired` error class:** the decision in §5.
