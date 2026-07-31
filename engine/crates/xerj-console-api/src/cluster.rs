//! Cluster awareness endpoints.
//!
//! Phase 1 ships only `/cluster/info` — the standalone-vs-raft probe the
//! Xerj Console SPA reads at boot to decide whether to render the topology
//! widget. Full RAFT endpoints (`/cluster/raft`, `/cluster/peers`,
//! `/cluster/replication`, admin actions) land in phase 4 once the
//! identity and prefs paths have been proven end-to-end.

use axum::{extract::State, response::Response};
use serde_json::json;

use crate::error::ConsoleResult;
use crate::response::ok;
use crate::state::ConsoleState;

/// `GET /_xerj-console/api/v1/cluster/info`
///
/// Returns enough for the SPA to:
/// - Show "node up since…" without a separate /uptime fetch.
/// - Render `mode == "standalone"` (single-node card, hide topology
///   widget) or `mode == "raft"` (will light up `/cluster/raft` etc.
///   in phase 4 — until then the SPA falls back to standalone view).
pub async fn info(State(state): State<ConsoleState>) -> ConsoleResult<Response> {
    use crate::state::ClusterMode;

    let mode = match state.cluster_mode {
        ClusterMode::Standalone => "standalone",
        ClusterMode::Raft => "raft",
    };

    Ok(ok(info_body(mode, state.started_at.0), None))
}

/// Build the unauthenticated `cluster/info` body.
///
/// AUTHZ-2: this endpoint is unauthenticated (the SPA reads it at boot, before
/// login, to choose standalone-vs-topology view). It must therefore expose only
/// what that decision needs — `mode` — plus the benign uptime. `node_id` and
/// the exact build `version` are deliberately omitted: a pre-auth version string
/// hands an attacker a precise target for known-CVE matching, and the node
/// identity leaks cluster topology. Authenticated callers get the full detail
/// from the credentialed cluster/status paths.
fn info_body(mode: &str, started_at_ms: i64) -> serde_json::Value {
    json!({
        "mode":          mode,
        "started_at_ms": started_at_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::info_body;

    #[test]
    fn info_body_omits_node_id_and_version_pre_auth() {
        let v = info_body("standalone", 1_700_000_000_000);
        assert_eq!(v.get("mode").and_then(|m| m.as_str()), Some("standalone"));
        assert!(v.get("started_at_ms").is_some());
        // The AUTHZ-2 disclosure fields must be gone from the pre-auth body.
        assert!(v.get("node_id").is_none(), "node_id must not leak pre-auth");
        assert!(v.get("version").is_none(), "version must not leak pre-auth");
        // And the whole serialized body must not contain the build version.
        let s = v.to_string();
        assert!(
            !s.contains(env!("CARGO_PKG_VERSION")),
            "build version must not appear in the pre-auth body: {s}"
        );
    }
}
