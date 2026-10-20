//! The hub reaches Postgres over verified TLS: it builds AgentStateGraph's
//! storage with the `postgres-tls` feature.
//!
//! `the_hub_build_has_postgres_tls` needs no database. The others need a
//! Postgres that accepts only TLS, named by `TEST_DATABASE_TLS_URL` (without
//! an `sslmode`) and `TEST_DATABASE_TLS_ROOTCERT` (the CA that signed its
//! certificate, which is valid for `localhost` only); without them they skip.
//! `scripts/test-postgres-tls.sh` starts one and runs them.

use agentstategraph_storage::PostgresStorage;
use ctxone_hub::hub_store::{HubStore, PgStore};

/// Without the feature, asking for TLS fails before connecting, with an error
/// naming the feature. With it, the same string gets as far as reading its
/// `sslrootcert`.
#[test]
fn the_hub_build_has_postgres_tls() {
    let err = PostgresStorage::connect_tenant_blocking(
        "postgres://u@127.0.0.1:1/db?sslmode=verify-full&sslrootcert=/nonexistent/ca.pem",
        "t",
        1,
    )
    .err()
    .expect("an unreadable sslrootcert is refused");
    let msg = format!("{err:?}");
    assert!(!msg.contains("postgres-tls"), "built without TLS: {msg}");
    assert!(msg.contains("sslrootcert"), "{msg}");
}

fn tls_server() -> Option<(String, String)> {
    match (
        std::env::var("TEST_DATABASE_TLS_URL"),
        std::env::var("TEST_DATABASE_TLS_ROOTCERT"),
    ) {
        (Ok(url), Ok(ca)) if !url.is_empty() && !ca.is_empty() => Some((url, ca)),
        _ => {
            eprintln!("skip: TEST_DATABASE_TLS_URL / TEST_DATABASE_TLS_ROOTCERT not set");
            None
        }
    }
}

fn tenant(tag: &str) -> String {
    format!("ctx-tls-{tag}-{}", uuid::Uuid::new_v4().simple())
}

fn connect(url: &str) -> Result<PostgresStorage, String> {
    PostgresStorage::connect_tenant_blocking(url, &tenant("t"), 2).map_err(|e| format!("{e:?}"))
}

/// `sslmode=verify-full` with the server's CA: the hub's tables work over it.
#[test]
fn a_hub_works_over_verified_tls() {
    let Some((url, ca)) = tls_server() else {
        return;
    };
    let storage = connect(&format!("{url}?sslmode=verify-full&sslrootcert={ca}"))
        .expect("verified TLS connects");
    let store = HubStore::Postgres(PgStore::new(&storage).expect("hub tables"));
    store
        .register_project("over-tls", None, "over-tls", None, None)
        .unwrap();
    let p = store
        .resolve_by_id("over-tls")
        .unwrap()
        .expect("registered");
    assert_eq!(p.namespace_id, "over-tls");
}

/// Plaintext is refused by a TLS-only server. (`sslmode=disable` rather than
/// none, so the CA test's `PGSSLROOTCERT` can't change the mode under it.)
#[test]
fn plaintext_is_refused_by_a_tls_only_server() {
    let Some((url, _)) = tls_server() else { return };
    let err = connect(&format!("{url}?sslmode=disable"))
        .err()
        .expect("plaintext refused");
    assert!(
        err.contains("pg_hba") || err.contains("encryption"),
        "{err}"
    );
}

/// A certificate from a CA the hub doesn't trust is refused (the test CA isn't
/// among Mozilla's roots), and `PGSSLROOTCERT` can name the CA instead of the
/// connection string. One test, because the variable is process-wide.
#[test]
fn an_untrusted_certificate_is_refused_until_pgsslrootcert_names_its_ca() {
    let Some((url, ca)) = tls_server() else {
        return;
    };
    let verify_full = format!("{url}?sslmode=verify-full");
    let err = connect(&verify_full).err().expect("untrusted CA refused");
    assert!(err.to_lowercase().contains("certificate"), "{err}");

    // SAFETY: no other test in this binary reads or writes PGSSLROOTCERT, and
    // the ones that connect with TLS name their CA in the connection string,
    // which wins over the variable.
    unsafe { std::env::set_var("PGSSLROOTCERT", &ca) };
    let connected = connect(&verify_full);
    unsafe { std::env::remove_var("PGSSLROOTCERT") };
    connected.expect("verified TLS connects with PGSSLROOTCERT");
}

/// The certificate must match the host name: it's valid for `localhost`, so
/// reaching the same server as 127.0.0.1 is refused.
#[test]
fn a_certificate_for_another_host_is_refused() {
    let Some((url, ca)) = tls_server() else {
        return;
    };
    let by_ip = url.replacen("@localhost", "@127.0.0.1", 1);
    assert_ne!(
        by_ip, url,
        "TEST_DATABASE_TLS_URL should name host localhost"
    );
    let err = connect(&format!("{by_ip}?sslmode=verify-full&sslrootcert={ca}"))
        .err()
        .expect("host name mismatch refused");
    assert!(err.to_lowercase().contains("certificate"), "{err}");
}
