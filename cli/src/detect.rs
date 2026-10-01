//! Asking the hub which workspace a directory belongs to.
//!
//! `GET /api/projects/detect` has three kinds of outcome, and they used to be
//! collapsed into two: a project was found, or "anything else" — which meant
//! the `default` workspace. A hub that timed out, errored or could not be
//! reached therefore looked exactly like a directory with no project, and
//! every command ran against `default` without a word. After a `brew upgrade`
//! on macOS that is what happened: the new hub binary sat blocked on a privacy
//! prompt reading `.ctxproject`, the 1.5 s client timeout fired, and plans and
//! branches came back "not found".
//!
//! [`detect`] keeps the three apart. `Ok(Some(ns))` and `Ok(None)` are answers;
//! `Err` means nobody knows, and callers must not quietly pick `default`.

use std::time::Duration;

/// How long to wait for the hub's answer.
///
/// Longer than the hub's own detect timeout (3 s) so that a stuck detection
/// comes back as the hub's 504, which explains itself, rather than as a bare
/// client timeout. A normal detection takes tens of milliseconds, so this only
/// ever costs time when something is already wrong.
pub const DETECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Why detection produced no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Could not connect to the hub at all.
    Unreachable,
    /// The hub (or the request to it) ran out of time.
    Timeout,
    /// The hub answered with an error, or with something unparseable.
    Hub,
    /// The current directory itself is gone (e.g. a deleted worktree).
    NoCwd,
}

/// Detection did not produce an answer. Deliberately not `Ok(None)`.
#[derive(Debug, Clone)]
pub struct DetectFailure {
    pub kind: FailureKind,
    /// The directory detection was asked about.
    pub cwd: String,
    /// What went wrong, in terms a user can act on.
    pub detail: String,
}

impl DetectFailure {
    pub fn new(kind: FailureKind, cwd: &str, detail: impl Into<String>) -> Self {
        Self {
            kind,
            cwd: cwd.to_string(),
            detail: detail.into(),
        }
    }

    fn from_reqwest(server: &str, cwd: &str, e: &reqwest::Error) -> Self {
        if e.is_timeout() {
            let mut detail = format!(
                "the hub at {server} did not answer within {}s",
                DETECT_TIMEOUT.as_secs()
            );
            if cfg!(target_os = "macos") {
                detail.push_str(
                    " — if macOS is showing a privacy prompt for ctxone-hub \
                     (common right after an upgrade), answer it",
                );
            }
            Self::new(FailureKind::Timeout, cwd, detail)
        } else if e.is_connect() {
            Self::new(
                FailureKind::Unreachable,
                cwd,
                format!("hub unreachable at {server} ({})", root_cause(e)),
            )
        } else {
            Self::new(
                FailureKind::Hub,
                cwd,
                format!("request to {server} failed ({e})"),
            )
        }
    }

    /// Exit status for a command that stops here (sysexits-style, matching
    /// the rest of the CLI).
    pub fn exit_code(&self) -> i32 {
        match self.kind {
            FailureKind::Unreachable => crate::EX_UNAVAILABLE,
            FailureKind::Timeout => crate::EX_TEMPFAIL,
            FailureKind::Hub => crate::EX_PROTOCOL,
            FailureKind::NoCwd => crate::EX_NOINPUT,
        }
    }

    /// The first line of the report: what could not be determined, and why.
    pub fn headline(&self) -> String {
        format!(
            "can't tell which workspace {} belongs to: {}",
            self.cwd, self.detail
        )
    }

    /// Print why detection failed and how to get past it.
    pub fn report(&self) {
        eprintln!("ctx: {}", self.headline());
        eprintln!("{}", NAMESPACE_HINT);
    }

    /// Report and stop. Running on in `default` would read and write the
    /// wrong workspace, which is worse than not running.
    pub fn exit(&self) -> ! {
        self.report();
        std::process::exit(self.exit_code());
    }
}

/// What to do instead, printed under every detection failure.
pub const NAMESPACE_HINT: &str = "  Not falling back to the 'default' workspace, which would read and \
     write the wrong data.\n  Pass --namespace <workspace> (or set CTX_NAMESPACE) to choose one \
     and skip detection.";

/// Ask the hub which workspace `cwd` belongs to.
///
/// `client` should carry a timeout (see [`DETECT_TIMEOUT`]) and whatever auth
/// the hub needs; this adds nothing to it.
pub async fn detect(
    client: &reqwest::Client,
    server: &str,
    cwd: &str,
) -> Result<Option<String>, DetectFailure> {
    let resp = client
        .get(format!("{server}/api/projects/detect"))
        .query(&[("cwd", cwd)])
        .send()
        .await
        .map_err(|e| DetectFailure::from_reqwest(server, cwd, &e))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| DetectFailure::from_reqwest(server, cwd, &e))?;
    interpret(status, &body, cwd)
}

/// Turn the hub's response into an answer or a failure.
///
/// Only `found`, `not_found` and `registry_unavailable` on a 2xx are answers.
/// Everything else — an error status, a body that does not parse, a status
/// string this build does not know — is a failure, because each of them is a
/// case where the directory might belong to a project nobody could check.
pub fn interpret(
    status: reqwest::StatusCode,
    body: &str,
    cwd: &str,
) -> Result<Option<String>, DetectFailure> {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    if !status.is_success() {
        // The hub explains its own failures; anything else (a proxy, a 401)
        // gets the status line so there is something to go on.
        let detail = parsed
            .as_ref()
            .and_then(|v| v["error"].as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("hub answered {status}: {}", snippet(body)));
        let kind = if status == reqwest::StatusCode::GATEWAY_TIMEOUT {
            FailureKind::Timeout
        } else {
            FailureKind::Hub
        };
        return Err(DetectFailure::new(kind, cwd, detail));
    }
    let unexpected = || {
        DetectFailure::new(
            FailureKind::Hub,
            cwd,
            format!("unexpected answer from the hub: {}", snippet(body)),
        )
    };
    let v = parsed.ok_or_else(unexpected)?;
    match v["status"].as_str() {
        Some("found") => v["namespace"]
            .as_str()
            .map(|ns| Some(ns.to_string()))
            .ok_or_else(unexpected),
        // `registry_unavailable` is a hub with no project registry at all
        // (memory/postgres backend): `default` is the only workspace it has.
        Some("not_found") | Some("registry_unavailable") => Ok(None),
        _ => Err(unexpected()),
    }
}

/// The innermost error in `e`'s chain (e.g. "Connection refused"), without the
/// request URL reqwest's own message repeats.
fn root_cause(e: &reqwest::Error) -> String {
    let mut cur: &dyn std::error::Error = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

/// A bounded, single-line excerpt of a response body for an error message.
fn snippet(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return "(empty body)".to_string();
    }
    match flat.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &flat[..i]),
        None => flat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    const CWD: &str = "/Users/me/Documents/repo";

    #[test]
    fn found_is_the_project_namespace() {
        let body = r#"{"status":"found","via":"ctxproject","project_id":"p","namespace":"p-ns"}"#;
        assert_eq!(
            interpret(StatusCode::OK, body, CWD).unwrap(),
            Some("p-ns".into())
        );
    }

    #[test]
    fn not_found_and_no_registry_are_the_default_workspace() {
        for status in ["not_found", "registry_unavailable"] {
            let body = format!(r#"{{"status":"{status}","namespace":"default"}}"#);
            assert_eq!(interpret(StatusCode::OK, &body, CWD).unwrap(), None);
        }
    }

    /// The hub's own detect timeout (504) is a timeout, and its explanation
    /// — the part that mentions the macOS prompt — reaches the user.
    #[test]
    fn hub_timeout_is_a_timeout_failure_with_its_explanation() {
        let body = r#"{"status":"timeout","error":"project detection did not finish within 3s"}"#;
        let f = interpret(StatusCode::GATEWAY_TIMEOUT, body, CWD).unwrap_err();
        assert_eq!(f.kind, FailureKind::Timeout);
        assert_eq!(f.exit_code(), crate::EX_TEMPFAIL);
        assert!(
            f.detail.contains("did not finish within 3s"),
            "{}",
            f.detail
        );
        assert!(f.headline().contains(CWD));
    }

    #[test]
    fn hub_errors_are_failures_not_not_found() {
        let body =
            r#"{"status":"error","error":"cannot read /r/.ctxproject: Operation not permitted"}"#;
        let f = interpret(StatusCode::INTERNAL_SERVER_ERROR, body, CWD).unwrap_err();
        assert_eq!(f.kind, FailureKind::Hub);
        assert!(f.detail.contains("Operation not permitted"));

        let f = interpret(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"status":"busy","error":"x"}"#,
            CWD,
        )
        .unwrap_err();
        assert_eq!(f.kind, FailureKind::Hub);

        // A plain-text error body (e.g. a 401 from an authenticated hub).
        let f = interpret(StatusCode::UNAUTHORIZED, "missing bearer token", CWD).unwrap_err();
        assert!(f.detail.contains("missing bearer token"), "{}", f.detail);
    }

    #[test]
    fn unparseable_or_unknown_answers_are_failures() {
        assert!(interpret(StatusCode::OK, "<html>proxy</html>", CWD).is_err());
        assert!(interpret(StatusCode::OK, r#"{"status":"maybe"}"#, CWD).is_err());
        assert!(interpret(StatusCode::OK, r#"{"status":"found"}"#, CWD).is_err());
    }

    #[test]
    fn snippet_is_bounded_and_single_line() {
        let long = "word ".repeat(100);
        let s = snippet(&long);
        assert!(s.chars().count() <= 201);
        assert!(!s.contains('\n'));
        assert_eq!(snippet("  "), "(empty body)");
    }

    fn client(timeout: Duration) -> reqwest::Client {
        reqwest::Client::builder().timeout(timeout).build().unwrap()
    }

    /// A hub that accepts the connection but never answers — an older hub
    /// stuck behind a macOS privacy prompt — is a timeout, not "no project".
    #[tokio::test]
    async fn silent_hub_is_a_timeout_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let hold = tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let f = detect(&client(Duration::from_millis(200)), &server, CWD)
            .await
            .unwrap_err();
        assert_eq!(f.kind, FailureKind::Timeout, "{f:?}");
        hold.abort();
    }

    #[tokio::test]
    async fn closed_port_is_an_unreachable_failure() {
        // Bind then drop, so the port is (almost certainly) closed.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let server = format!("http://127.0.0.1:{port}");
        let f = detect(&client(Duration::from_secs(2)), &server, CWD)
            .await
            .unwrap_err();
        assert_eq!(f.kind, FailureKind::Unreachable, "{f:?}");
        assert_eq!(f.exit_code(), crate::EX_UNAVAILABLE);
    }

    /// End to end over a real socket: a hub answering `not_found` is `Ok(None)`.
    #[tokio::test]
    async fn answering_hub_round_trips() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let serve = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let body = r#"{"status":"not_found","namespace":"default"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        });
        let got = detect(&client(Duration::from_secs(2)), &server, CWD).await;
        assert_eq!(got.unwrap(), None);
        serve.await.unwrap();
    }
}
