//! A credential scan on every push (ctx-hub-sync t-007).
//!
//! AgentStateGraph runs each push's new values through its push filters
//! before anything leaves the store. Its own `CredentialFilter` always runs;
//! [`SecretsPushFilter`] adds CTXone's rules, the ones `ctx secrets scan`
//! uses ([`crate::secrets`]). A high-confidence hit (a provider's token
//! format, or a private key) blocks the push. Medium-confidence shapes,
//! which ordinary text sometimes takes too, don't: `ctx secrets scan` reports
//! those.
//!
//! A finding never carries the value. One the user has checked is let
//! through with [`PushOptions::allow`] and a reason, which the remote's push
//! log keeps. Anything under `/_meta/_secret` can never be let through;
//! AgentStateGraph refuses it whatever the options say.

use std::sync::Arc;

use agentstategraph::sync::{Finding, PushFilter, PushOptions, PushOverride};
use serde_json::json;

use crate::secrets::{Confidence, scan_text};

/// The name findings from CTXone's rules are reported under.
pub const FILTER_NAME: &str = "ctxone-secrets";

/// CTXone's credential rules, as an AgentStateGraph push filter.
pub struct SecretsPushFilter;

impl PushFilter for SecretsPushFilter {
    fn name(&self) -> &str {
        FILTER_NAME
    }

    fn check(&self, _path: &str, value: &str) -> Vec<String> {
        scan_text(value)
            .into_iter()
            .filter(|hit| hit.rule.confidence == Confidence::High)
            .map(|hit| hit.rule.id.to_string())
            .collect()
    }
}

/// The options every CTXone push goes out with: AgentStateGraph's credential
/// filter and CTXone's, the overrides the user gave, and the commits the
/// remote's history starts at.
pub fn push_options(
    shallow: Vec<agentstategraph_core::ObjectId>,
    overrides: Vec<PushOverride>,
) -> PushOptions {
    let mut options = PushOptions::default().with_filter(Arc::new(SecretsPushFilter));
    options.shallow = shallow;
    options.overrides = overrides;
    options
}

/// A finding as a client shows it: where it is, which rule, whether it can
/// be let through, and, where there is one, the record to forget to remove
/// it from the branch.
pub fn finding_json(finding: &Finding) -> serde_json::Value {
    json!({
        "id": finding.id,
        "filter": finding.filter,
        "rule": finding.rule,
        "path": finding.path,
        "commit": finding.commit.short(),
        "overridable": finding.overridable,
        "forget": crate::secrets::forget_target(&finding.path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstategraph::sync::InProcessRemote;
    use agentstategraph::{CommitOptions, Repository};
    use agentstategraph_core::IntentCategory;
    use agentstategraph_storage::MemoryStorage;

    fn opts() -> CommitOptions {
        CommitOptions::new("test", IntentCategory::Checkpoint, "write")
    }

    /// A provider-shaped token, put together at run time so no source file
    /// holds one.
    fn token() -> String {
        let tail: String = (0..36)
            .map(|i| match i % 3 {
                0 => 'a',
                1 => '7',
                _ => 'Q',
            })
            .collect();
        format!("ghp_{tail}")
    }

    #[test]
    fn only_high_confidence_hits_are_rules() {
        let f = SecretsPushFilter;
        assert_eq!(
            f.check("/memory/ci/f1", &format!("CI uses {}", token())),
            vec!["github-token".to_string()]
        );
        // A secret-named value is medium confidence: reported by
        // `ctx secrets scan`, not a reason to stop a push.
        assert!(
            f.check("/memory/x", "api_key = hunter2-not-real")
                .is_empty()
        );
        assert!(f.check("/memory/x", "nothing here").is_empty());
    }

    #[test]
    fn a_credential_blocks_the_push_until_it_is_let_through() {
        let a = Repository::new(Box::new(MemoryStorage::new()));
        a.init().unwrap();
        a.set_json(
            "main",
            "/memory/ci/f1",
            &json!(format!("CI uses {}", token())),
            opts(),
        )
        .unwrap();
        let hub = Repository::new(Box::new(MemoryStorage::new()));
        hub.init().unwrap();
        let remote = InProcessRemote::new(&hub);
        a.fetch("hub", &remote, &[]).unwrap();

        let err = a
            .push_with(
                "hub",
                &remote,
                "main",
                &push_options(Vec::new(), Vec::new()),
            )
            .unwrap_err();
        let agentstategraph::sync::SyncError::Blocked { findings, .. } = err else {
            panic!("expected a blocked push, got {err:?}");
        };
        let ours: Vec<&Finding> = findings
            .iter()
            .filter(|f| f.filter == FILTER_NAME)
            .collect();
        assert_eq!(ours.len(), 1, "{findings:?}");
        let shown = finding_json(ours[0]);
        assert_eq!(shown["path"], "/memory/ci/f1");
        assert_eq!(shown["forget"], "/memory/ci/f1");
        assert!(!shown.to_string().contains(&token()), "never the value");
        assert!(hub.get("main", "/memory").is_err());

        // Every finding let through, the push lands.
        let overrides = findings
            .iter()
            .map(|f| PushOverride {
                finding: f.id.clone(),
                reason: "a test fixture".into(),
            })
            .collect();
        a.push_with("hub", &remote, "main", &push_options(Vec::new(), overrides))
            .unwrap();
        assert!(hub.get("main", "/memory/ci/f1").is_ok());
    }
}
