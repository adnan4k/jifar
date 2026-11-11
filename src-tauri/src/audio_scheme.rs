//! `jifar-audio://` URI scheme: lets the WebView play a meeting's audio
//! without filesystem access (SPEC §6.1, PLAN §6.2 option b).
//!
//! Scope is deliberately narrow and read-only: only `mixed.opus`,
//! `mic.opus` and `system.opus` inside `meetings/<uuid>/`. Requests look like
//! `jifar-audio://localhost/<meetingId>%2F<file>` (from `convertFileSrc`).
//! HTTP range requests are honoured. The UI fetches the file and plays it
//! from a `blob:` URL: WebKitGTK's media pipeline (GStreamer) can't load
//! custom schemes itself, but `fetch` can. The response is CORS-readable;
//! custom schemes are reachable only from this app's WebView.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use tauri::http::{Request, Response, StatusCode, header};
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use crate::AppState;

pub const SCHEME: &str = "jifar-audio";
const FILES: [&str; 3] = ["mixed.opus", "mic.opus", "system.opus"];
/// Largest body per response; the player asks for the rest with ranges.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;

pub fn handle<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let data_dir = ctx.app_handle().state::<AppState>().paths.meetings_dir();
    // File reads can block; don't hold up the WebView's thread.
    std::thread::spawn(move || responder.respond(serve(&data_dir, &request)));
}

fn serve(meetings_dir: &std::path::Path, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(path) = resolve(meetings_dir, request.uri().path()) else {
        tracing::debug!(path = request.uri().path(), "audio request out of scope");
        return status(StatusCode::NOT_FOUND);
    };
    tracing::debug!(file = %path.display(), "serving meeting audio");
    let Ok(mut file) = File::open(&path) else {
        return status(StatusCode::NOT_FOUND);
    };
    let Ok(size) = file.metadata().map(|m| m.len()) else {
        return status(StatusCode::INTERNAL_SERVER_ERROR);
    };
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, size));
    let (start, end, partial) = match range {
        Some((s, e)) => (s, e.min(s + MAX_CHUNK - 1), true),
        None if size == 0 => (0, 0, false),
        None => (0, (size - 1).min(MAX_CHUNK - 1), size > MAX_CHUNK),
    };
    let len = if size == 0 { 0 } else { end - start + 1 };
    let mut body = vec![0u8; usize::try_from(len).unwrap_or(0)];
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut body).is_err() {
        return status(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut res = Response::builder()
        .header(header::CONTENT_TYPE, "audio/ogg")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, len.to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    if partial {
        res = res
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    res.body(body)
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR))
}

/// Maps a request path to a file, or `None` if it is outside the scope.
fn resolve(meetings_dir: &std::path::Path, uri_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(uri_path.trim_start_matches('/'))?;
    let (id, file) = decoded.split_once('/')?;
    let id_ok = id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    (id_ok && FILES.contains(&file)).then(|| meetings_dir.join(id).join(file))
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` → inclusive (start, end).
fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let spec = value.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (size.saturating_sub(n), size - 1)
        }
        (a, "") => (a.parse().ok()?, size - 1),
        (a, b) => (a.parse().ok()?, b.parse::<u64>().ok()?.min(size - 1)),
    };
    (start <= end && start < size).then_some((start, end))
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    let mut res = Response::new(Vec::new());
    *res.status_mut() = code;
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01a1cb9c-1724-767b-ba9e-fc64a9d1e92b";

    #[test]
    fn only_meeting_audio_files_resolve() {
        let dir = std::path::Path::new("/data/meetings");
        assert_eq!(
            resolve(dir, &format!("/{ID}%2Fmixed.opus")),
            Some(dir.join(ID).join("mixed.opus"))
        );
        assert!(resolve(dir, &format!("/{ID}%2Fmic.opus")).is_some());
        // Anything else is out of scope.
        assert!(resolve(dir, &format!("/{ID}%2Fchunks%2Fmic-0000.wav")).is_none());
        assert!(resolve(dir, "/..%2F..%2Fjifar.db").is_none());
        assert!(resolve(dir, &format!("/..%2F{ID}%2Fmixed.opus")).is_none());
        assert!(resolve(dir, "/not-a-uuid%2Fmixed.opus").is_none());
        assert!(resolve(dir, &format!("/{ID}%2F..%2F..%2Fsettings.json")).is_none());
    }

    #[test]
    fn parses_ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=900-5000", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
    }

    #[test]
    fn serves_partial_content() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(ID)).unwrap();
        std::fs::write(tmp.path().join(ID).join("mixed.opus"), b"0123456789").unwrap();
        let req = Request::builder()
            .uri(format!("jifar-audio://localhost/{ID}%2Fmixed.opus"))
            .header(header::RANGE, "bytes=2-5")
            .body(Vec::new())
            .unwrap();
        let res = serve(tmp.path(), &req);
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body(), b"2345");
        assert_eq!(res.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
        let whole = Request::builder()
            .uri(format!("jifar-audio://localhost/{ID}%2Fmixed.opus"))
            .body(Vec::new())
            .unwrap();
        let res = serve(tmp.path(), &whole);
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.body(), b"0123456789");
    }
}
