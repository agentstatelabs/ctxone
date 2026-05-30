//! HTTP helpers for the CTX-hub → ASD code-intelligence proxy.
//!
//! These functions are called by the MCP tool methods on `CtxOneServer`.
//! They resolve a repo name to a base URL from the registry, then forward
//! requests to the matching ASD HTTP server and return the raw JSON body.

/// Resolve a repo name to its ASD base URL.
///
/// `repo` is the caller-supplied name (optional). Resolution order:
///   1. Exact match on name in `asd_repos`.
///   2. If None/empty and exactly one repo is registered, use it.
///   3. Error.
pub fn resolve_base<'a>(
    asd_repos: &'a [(String, String)],
    repo: Option<&str>,
) -> Result<&'a str, String> {
    if asd_repos.is_empty() {
        return Err(
            "No ASD repos registered. Start ctxone-hub with --asd-url name=http://... to enable code tools."
                .to_string(),
        );
    }
    match repo {
        Some(name) if !name.is_empty() => asd_repos
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, u)| u.as_str())
            .ok_or_else(|| {
                let names: Vec<&str> = asd_repos.iter().map(|(n, _)| n.as_str()).collect();
                format!(
                    "unknown repo \"{}\". Known repos: {}",
                    name,
                    names.join(", ")
                )
            }),
        _ => {
            if asd_repos.len() == 1 {
                Ok(asd_repos[0].1.as_str())
            } else {
                let names: Vec<&str> = asd_repos.iter().map(|(n, _)| n.as_str()).collect();
                Err(format!(
                    "Multiple repos registered; specify repo name. Known repos: {}",
                    names.join(", ")
                ))
            }
        }
    }
}

/// GET <base>/api/v1/<path> and return the response body as a String.
pub async fn asd_get(base: &str, path: &str) -> Result<String, String> {
    let url = format!("{}/api/v1/{}", base.trim_end_matches('/'), path);
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("ASD request failed: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("ASD returned {}: {}", resp.status(), url));
    }
    resp.text().await.map_err(|e| e.to_string())
}

/// List all registered repos as JSON.
pub fn list_repos_json(asd_repos: &[(String, String)]) -> String {
    let v: Vec<serde_json::Value> = asd_repos
        .iter()
        .map(|(name, url)| serde_json::json!({ "name": name, "url": url }))
        .collect();
    serde_json::to_string(&v).unwrap_or_else(|_| "[]".to_string())
}
