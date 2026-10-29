//! A Postgres role and a schema of its own, for tests that run with row-level
//! security on (ctx-multi-tenant-hub t-011).
//!
//! The role is neither a superuser nor `BYPASSRLS`, so row-level security
//! binds it, and it owns the schema, so a Hub connecting as it creates its
//! tables there as their owner: the policies bind it only because they're
//! forced. The test's own `TEST_DATABASE_URL` role sets it up, so that role
//! needs to be able to create roles (CI's is a superuser).

// Each test binary that includes this uses only some of it.
#![allow(dead_code)]

use agentstategraph_storage::tokio_postgres;

/// The role a Hub connects as. Roles are global to the cluster, so it's
/// shared by every test and created once.
pub const ROLE: &str = "ctxone_row_security_test";
const PASSWORD: &str = "ctxone-row-security-test";

/// A fresh schema the role owns.
pub struct Schema {
    pub name: String,
    /// Into the schema as the role.
    pub app_url: String,
    /// Into the schema as the test's own role, which skips row-level
    /// security, so it sees every tenant's rows.
    pub admin_url: String,
    admin: tokio_postgres::Client,
}

impl Schema {
    /// Create the role unless it exists, and a schema named for `tag` that it
    /// owns, through `admin_url` (the test's `TEST_DATABASE_URL`).
    pub async fn new(admin_url: &str, tag: &str) -> Self {
        let name = format!(
            "ctx_rls_{tag}_{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        );
        let admin = connect(admin_url).await;
        assert!(
            bypasses_row_security(&admin).await,
            "TEST_DATABASE_URL's role must be a superuser, to create the test role"
        );
        // Two first runs at once can collide on the catalog's unique index
        // rather than see the role already there.
        admin
            .batch_execute(&format!(
                "DO $$ BEGIN
                     CREATE ROLE {ROLE} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '{PASSWORD}';
                 EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
                 END $$;
                 CREATE SCHEMA {name} AUTHORIZATION {ROLE};"
            ))
            .await
            .expect("create the role and schema");
        Self {
            app_url: role_url(admin_url, &name),
            admin_url: in_schema(admin_url, &name),
            name,
            admin,
        }
    }

    /// Drop the schema and everything in it.
    pub async fn drop(self) {
        self.admin
            .batch_execute(&format!("DROP SCHEMA {} CASCADE", self.name))
            .await
            .expect("drop the schema");
    }
}

/// Switch row-level security on for the database at `url` with
/// `ctxone-hub db row-security on`, which sets the Hub's tables up first, and
/// check that it binds `url`'s role. Blocks: call it from a blocking thread.
pub fn switch_on(url: &str) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = {
        let mut console = ctxone_hub::tenant_admin::Console {
            out: &mut out,
            err: &mut err,
            ask: None,
        };
        let args = ["row-security", "on", "--database-url", url].map(String::from);
        ctxone_hub::db_admin::run(&args, &mut console)
    };
    let (out, err) = (String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
    assert_eq!((code, out.as_ref()), (0, "on\n"), "{err}");
    assert!(!err.contains("BYPASSRLS"), "{err}");
    assert!(!err.contains("no tenant policy"), "{err}");
}

/// A connection of the test's own.
pub async fn connect(url: &str) -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .expect("connect");
    tokio::spawn(connection);
    client
}

/// Whether `client`'s role skips row-level security.
pub async fn bypasses_row_security(client: &tokio_postgres::Client) -> bool {
    client
        .query_one(
            "SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user",
            &[],
        )
        .await
        .expect("read the role")
        .get(0)
}

/// `url` with `schema` alone on its search path.
fn in_schema(url: &str, schema: &str) -> String {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        let sep = if url.contains('?') { '&' } else { '?' };
        format!("{url}{sep}options=-c%20search_path%3D{schema}")
    } else {
        format!("{url} options='-c search_path={schema}'")
    }
}

/// `admin_url`, logging in as [`ROLE`] instead, with `schema` alone on its
/// search path.
fn role_url(admin_url: &str, schema: &str) -> String {
    let url = if let Some((scheme, rest)) = admin_url.split_once("://") {
        let (authority, path) = match rest.find('/') {
            Some(slash) => rest.split_at(slash),
            None => (rest, ""),
        };
        let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        format!("{scheme}://{ROLE}:{PASSWORD}@{host}{path}")
    } else {
        // Later keys win in the key/value form.
        format!("{admin_url} user={ROLE} password={PASSWORD}")
    };
    in_schema(&url, schema)
}
