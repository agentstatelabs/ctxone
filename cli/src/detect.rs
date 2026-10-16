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
//!
//! `ctx` reads the directory itself — the nearest `.ctxproject` and the git
//! remote, as [`Clues`] — and the hub only looks them up in its registry. A hub
//! on another machine can't read the directory, so before this only a
//! registered `local_path` could match there.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long to wait for the hub's answer.
///
/// Longer than the hub's own detect timeout (3 s) so that a stuck detection
/// comes back as the hub's 504, which explains itself, rather than as a bare
/// client timeout. A normal detection takes tens of milliseconds, so this only
/// ever costs time when something is already wrong.
pub const DETECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long reading the directory's [`Clues`] may take. Milliseconds normally;
/// longer means the read is blocked, as on macOS behind an unanswered privacy
/// prompt (the hub's own 3 s detect bound exists for the same reason).
pub const CLUES_TIMEOUT: Duration = Duration::from_secs(3);

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

/// What a directory says about its project, read where `ctx` runs: steps 1
/// and 2 of the hub's detection chain. Mirrors `Clues` in the hub's
/// `server/src/project.rs`, which reads them itself for older clients.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Clues {
    /// The project id in the nearest `.ctxproject` (its first non-empty line).
    pub ctxproject: Option<String>,
    /// Why the nearest `.ctxproject` exists but couldn't be read.
    pub ctxproject_error: Option<String>,
    /// The git remote `origin`.
    pub remote_url: Option<String>,
}

impl Clues {
    /// Read `cwd`'s clues. Blocking: file reads and a `git` run.
    pub fn read(cwd: &Path) -> Self {
        let (ctxproject, ctxproject_error) = match find_ctxproject(cwd) {
            Ok(id) => (id, None),
            Err((path, e)) => (None, Some(unreadable_marker_message(&path, &e))),
        };
        Self {
            ctxproject,
            ctxproject_error,
            remote_url: crate::read_git_remote(cwd),
        }
    }

    /// The query for `GET /api/projects/detect` about `cwd`. `client_read`
    /// tells the hub to use these instead of reading `cwd` itself; a hub that
    /// predates it ignores them and reads `cwd` as before.
    pub fn query(&self, cwd: &str) -> Vec<(&'static str, String)> {
        let mut q = vec![("cwd", cwd.to_string()), ("client_read", "true".into())];
        let fields = [
            ("ctxproject", &self.ctxproject),
            ("ctxproject_error", &self.ctxproject_error),
            ("remote_url", &self.remote_url),
        ];
        for (name, value) in fields {
            if let Some(v) = value {
                q.push((name, v.clone()));
            }
        }
        q
    }
}

/// [`Clues::read`] off the async runtime, bounded by [`CLUES_TIMEOUT`]: a read
/// stuck behind a privacy prompt is a timeout failure, not a hang. (When the
/// hub's background sweep runs `ctx`, nobody may be there to answer it.)
pub async fn read_clues(cwd: &str) -> Result<Clues, DetectFailure> {
    let dir = PathBuf::from(cwd);
    let read = tokio::task::spawn_blocking(move || Clues::read(&dir));
    match tokio::time::timeout(CLUES_TIMEOUT, read).await {
        Ok(Ok(clues)) => Ok(clues),
        Ok(Err(e)) => Err(DetectFailure::new(
            FailureKind::Hub,
            cwd,
            format!("reading the directory failed ({e})"),
        )),
        Err(_) => {
            let mut detail = format!(
                "reading {cwd} (its .ctxproject and git remote) took over {}s",
                CLUES_TIMEOUT.as_secs()
            );
            if cfg!(target_os = "macos") {
                detail.push_str(" — if macOS is showing a privacy prompt, answer it");
            }
            Err(DetectFailure::new(FailureKind::Timeout, cwd, detail))
        }
    }
}

/// The detect query for `cwd`: its [`Clues`], read here.
pub async fn query_for(cwd: &str) -> Result<Vec<(&'static str, String)>, DetectFailure> {
    Ok(read_clues(cwd).await?.query(cwd))
}

/// Walk `start` and its parents for a `.ctxproject` and return its project id.
///
/// The same walk as the hub's `find_ctxproject_file`: the nearest marker with
/// a non-empty line wins, and one this process may not read stops the walk
/// with its path, because it may well be the nearest.
fn find_ctxproject(start: &Path) -> Result<Option<String>, (PathBuf, std::io::Error)> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(".ctxproject");
        let content = match std::fs::metadata(&candidate) {
            Ok(meta) if meta.is_file() => std::fs::read_to_string(&candidate),
            Ok(_) => Err(std::io::ErrorKind::InvalidInput.into()),
            Err(e) => Err(e),
        };
        match content {
            Ok(content) => {
                if let Some(id) = content.lines().map(str::trim).find(|l| !l.is_empty()) {
                    return Ok(Some(id.to_string()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err((candidate, e));
            }
            Err(_) => {}
        }
        dir = d.parent();
    }
    Ok(None)
}

/// Explain a `.ctxproject` this process may not read.
fn unreadable_marker_message(path: &Path, e: &std::io::Error) -> String {
    let mut msg = format!("cannot read {}: {e}", path.display());
    if cfg!(target_os = "macos") {
        msg.push_str(
            " — grant the app running ctx (your terminal, or ctxone-hub for its \
             background sync) access in System Settings → Privacy & Security → \
             Files and Folders (or Full Disk Access), then retry",
        );
    }
    msg
}

/// Ask the hub which workspace `cwd` belongs to.
///
/// Reads the directory's [`Clues`] here and sends them, so the hub only looks
/// them up. `client` should carry a timeout (see [`DETECT_TIMEOUT`]) and
/// whatever auth the hub needs; this adds nothing to it.
pub async fn detect(
    client: &reqwest::Client,
    server: &str,
    cwd: &str,
) -> Result<Option<String>, DetectFailure> {
    let query = query_for(cwd).await?;
    let resp = client
        .get(format!("{server}/api/projects/detect"))
        .query(&query)
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

    #[test]
    fn clues_come_from_the_nearest_marker_with_an_id() {
        let root = tempfile::tempdir().unwrap();
        let outer = root.path().join("outer");
        let inner = outer.join("inner");
        let deep = inner.join("src").join("deep");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(outer.join(".ctxproject"), "outer-id\n").unwrap();

        assert_eq!(Clues::read(&deep).ctxproject.as_deref(), Some("outer-id"));
        // A marker with only blank lines doesn't count; the walk goes on.
        std::fs::write(inner.join(".ctxproject"), "  \n\n").unwrap();
        assert_eq!(Clues::read(&deep).ctxproject.as_deref(), Some("outer-id"));
        // The nearest one with an id wins, trimmed.
        std::fs::write(inner.join(".ctxproject"), "\n  inner-id  \nignored\n").unwrap();
        let clues = Clues::read(&deep);
        assert_eq!(clues.ctxproject.as_deref(), Some("inner-id"));
        assert_eq!(clues.ctxproject_error, None);
    }

    /// A marker this process may not read is reported, and stops the walk:
    /// it may be the nearest.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_marker_is_reported_not_skipped() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let inner = root.path().join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(root.path().join(".ctxproject"), "outer-id\n").unwrap();
        let marker = inner.join(".ctxproject");
        std::fs::write(&marker, "inner-id\n").unwrap();
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_to_string(&marker).is_ok() {
            return; // running as root: permissions cannot hide the file
        }
        let clues = Clues::read(&inner);
        assert_eq!(clues.ctxproject, None);
        let err = clues.ctxproject_error.expect("an error");
        assert!(err.contains(&marker.display().to_string()), "{err}");
    }

    #[test]
    fn clues_carry_the_git_remote() {
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q"]);
        assert_eq!(Clues::read(repo.path()).remote_url, None);
        git(&[
            "remote",
            "add",
            "origin",
            "https://git.example.com/g/app.git",
        ]);
        let sub = repo.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert_eq!(
            Clues::read(&sub).remote_url.as_deref(),
            Some("https://git.example.com/g/app.git")
        );
    }

    #[test]
    fn the_query_says_the_client_read_the_directory() {
        let none = Clues::default().query(CWD);
        assert_eq!(
            none,
            [
                ("cwd", CWD.to_string()),
                ("client_read", "true".to_string())
            ]
        );
        let all = Clues {
            ctxproject: Some("p".into()),
            ctxproject_error: Some("e".into()),
            remote_url: Some("https://r".into()),
        }
        .query(CWD);
        for (k, v) in [
            ("ctxproject", "p"),
            ("ctxproject_error", "e"),
            ("remote_url", "https://r"),
        ] {
            assert!(all.contains(&(k, v.to_string())), "{all:?}");
        }
    }

    /// Over a real socket: `detect` reads the directory and sends what it
    /// found, so a hub that can't see the directory can still answer.
    #[tokio::test]
    async fn detect_sends_the_directorys_clues() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".ctxproject"), "my-proj\n").unwrap();
        let cwd = dir.path().to_str().unwrap().to_string();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let serve = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            let body = r#"{"status":"found","via":"ctxproject","project_id":"my-proj","namespace":"my-ns"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            request
        });
        let got = detect(&client(Duration::from_secs(2)), &server, &cwd).await;
        assert_eq!(got.unwrap(), Some("my-ns".into()));
        let request_line = serve.await.unwrap().lines().next().unwrap().to_string();
        assert!(
            request_line.starts_with("GET /api/projects/detect?"),
            "{request_line}"
        );
        assert!(request_line.contains("client_read=true"), "{request_line}");
        assert!(
            request_line.contains("ctxproject=my-proj"),
            "{request_line}"
        );
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
