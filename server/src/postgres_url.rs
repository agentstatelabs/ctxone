//! What a Postgres connection string says about TLS, for the hub's startup
//! log. AgentStateGraph's storage does the connecting and the verifying (its
//! `postgres-tls` feature, which the hub always builds with); this only reads
//! the string, so an operator can see whether a database on another machine
//! may be reached in plaintext without anyone having chosen that.
//!
//! The rules are the storage's: `sslmode=require`, `verify-ca` and
//! `verify-full` mean verified TLS (against Mozilla's roots, or `sslrootcert` /
//! `PGSSLROOTCERT`); `prefer` uses TLS only when the server offers it; no
//! `sslmode`, or `disable`, is plaintext.

/// How a connection string asks for TLS, and whether its database is local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsIntent {
    /// The `sslmode` the string names, lowercased; `None` when it names none.
    pub sslmode: Option<String>,
    /// Whether the database is on this machine: a loopback host or a Unix
    /// socket (including no host at all, libpq's default).
    pub local: bool,
}

impl TlsIntent {
    /// Read a URL (`postgres://…?sslmode=…`) or a key/value string
    /// (`host=… sslmode=…`).
    pub fn of(conn: &str) -> Self {
        let (host, params) = match conn
            .strip_prefix("postgres://")
            .or_else(|| conn.strip_prefix("postgresql://"))
        {
            Some(rest) => url_parts(rest),
            None => keyword_parts(conn),
        };
        // A `host` parameter overrides the URL's host, as in libpq.
        let host = param(&params, "host").unwrap_or(host);
        Self {
            sslmode: param(&params, "sslmode").map(|m| m.to_ascii_lowercase()),
            local: is_local(&host),
        }
    }

    /// The connection is TLS, verified: `require`, `verify-ca` or `verify-full`.
    pub fn verified(&self) -> bool {
        matches!(
            self.sslmode.as_deref(),
            Some("require" | "verify-ca" | "verify-full")
        )
    }

    /// The startup log's `tls` field.
    pub fn label(&self) -> &str {
        match self.sslmode.as_deref() {
            None | Some("disable") => "off",
            Some(mode) => mode,
        }
    }

    /// A warning when a database on another machine may be reached in
    /// plaintext and the connection string doesn't say that's intended. An
    /// explicit `sslmode=disable` (say, on a private Docker network) is a
    /// choice, so it isn't warned about.
    pub fn warning(&self) -> Option<String> {
        if self.local || self.verified() || self.sslmode.as_deref() == Some("disable") {
            return None;
        }
        Some(format!(
            "the Postgres database is on another machine, and its connection string doesn't \
             require TLS (sslmode={}), so it may be reached in plaintext: add \
             sslmode=verify-full (plus sslrootcert=<CA file> if the provider uses its own CA), \
             or sslmode=disable if plaintext on this network is intended",
            self.sslmode.as_deref().unwrap_or("unset")
        ))
    }
}

type Params = Vec<(String, String)>;

fn param(params: &Params, key: &str) -> Option<String> {
    params
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

/// The first host and the query parameters of a URL, after its scheme.
fn url_parts(rest: &str) -> (String, Params) {
    let (before_query, query) = rest.split_once('?').unwrap_or((rest, ""));
    let authority = before_query.split('/').next().unwrap_or("");
    let hosts = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let first = hosts.split(',').next().unwrap_or("");
    let host = match first.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or("").to_string(),
        None => first.split(':').next().unwrap_or("").to_string(),
    };
    let params = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), percent_decode(v)))
        .collect();
    (percent_decode(&host), params)
}

/// The `host` and every `key=value` of a libpq keyword string. Values may be
/// single-quoted.
fn keyword_parts(conn: &str) -> (String, Params) {
    let mut params = Vec::new();
    let mut rest = conn.trim_start();
    while let Some((key, after)) = rest.split_once('=') {
        let key = key.trim().to_string();
        let after = after.trim_start();
        let (value, tail) = match after.strip_prefix('\'') {
            Some(quoted) => match quoted.split_once('\'') {
                Some((v, t)) => (v, t),
                None => (quoted, ""),
            },
            None => after.split_once(char::is_whitespace).unwrap_or((after, "")),
        };
        params.push((key, value.to_string()));
        rest = tail.trim_start();
    }
    (String::new(), params)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Loopback, a Unix socket directory, or no host at all.
fn is_local(host: &str) -> bool {
    let host = host.split(',').next().unwrap_or("").trim();
    host.is_empty()
        || host.starts_with('/')
        || host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_modes_are_not_warned_about() {
        for mode in ["require", "verify-ca", "verify-full", "VERIFY-FULL"] {
            let t = TlsIntent::of(&format!(
                "postgres://u:p@db.example.com:5432/app?sslmode={mode}"
            ));
            assert!(t.verified(), "{mode}");
            assert!(!t.local);
            assert_eq!(t.warning(), None, "{mode}");
        }
        let t = TlsIntent::of(
            "postgres://u@db.example.com/app?sslrootcert=%2Fetc%2Fca.pem&sslmode=verify-full",
        );
        assert_eq!(t.label(), "verify-full");
    }

    #[test]
    fn a_remote_database_without_required_tls_is_warned_about() {
        for (url, label) in [
            ("postgres://u:p@db.example.com/app", "off"),
            ("postgres://u@10.0.0.5/app?sslmode=prefer", "prefer"),
            (
                "postgresql://db.example.com:6543/app?sslmode=allow",
                "allow",
            ),
            ("postgres://u@[2001:db8::1]:5432/app", "off"),
            ("postgres://u@db1.example.com,db2.example.com/app", "off"),
        ] {
            let t = TlsIntent::of(url);
            assert!(!t.local, "{url}");
            assert_eq!(t.label(), label, "{url}");
            let w = t
                .warning()
                .unwrap_or_else(|| panic!("no warning for {url}"));
            assert!(w.contains("sslmode=verify-full"), "{w}");
        }
    }

    /// Plaintext asked for by name is a choice, not an oversight.
    #[test]
    fn an_explicit_sslmode_disable_is_not_warned_about() {
        let t = TlsIntent::of("postgres://ctxone:secret@db:5432/ctxone?sslmode=disable");
        assert!(!t.local);
        assert_eq!(t.label(), "off");
        assert_eq!(t.warning(), None);
    }

    #[test]
    fn a_local_database_is_never_warned_about() {
        for url in [
            "postgres://asg:asg@localhost:5432/asg_test",
            "postgres://asg@127.0.0.1/db",
            "postgres://asg@[::1]:5432/db",
            "postgres:///db",
            "postgres://%2Fvar%2Frun%2Fpostgresql/db",
            "postgres://u@/db?host=/tmp",
            "host=localhost dbname=app",
            "dbname=app user=me",
        ] {
            let t = TlsIntent::of(url);
            assert!(t.local, "{url}");
            assert_eq!(t.warning(), None, "{url}");
        }
    }

    #[test]
    fn keyword_strings_are_read_too() {
        let t = TlsIntent::of("host=db.example.com port=5432 dbname='my app' sslmode=verify-full");
        assert!(!t.local);
        assert!(t.verified());
        let t = TlsIntent::of("host=db.example.com dbname=app");
        assert!(t.warning().is_some());
    }

    /// The query's `host` overrides the URL's, as in libpq.
    #[test]
    fn a_host_parameter_overrides_the_urls() {
        assert!(!TlsIntent::of("postgres://localhost/db?host=db.example.com").local);
        assert!(TlsIntent::of("postgres://db.example.com/db?host=%2Ftmp").local);
    }
}
