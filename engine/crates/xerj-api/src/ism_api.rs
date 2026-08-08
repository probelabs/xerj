//! OpenSearch Index State Management (ISM) — the native REST surface over
//! `xerj_engine::lifecycle`'s execution engine. See that module's doc
//! comment for why ISM's state machine (not ES ILM's fixed phases) is the
//! internal model both `_plugins/_ism/*` (here) and `_ilm/*`
//! (`es_compat::put_ilm_policy` and friends) are built on.
//!
//! Endpoints:
//!   PUT/GET/DELETE /_plugins/_ism/policies/{policy_id}
//!   GET            /_plugins/_ism/policies            (list/search — added
//!                  after live OSD UI verification showed the "State
//!                  management policies" table 500ing without it)
//!   POST           /_plugins/_ism/add/{index}
//!   GET            /_plugins/_ism/explain/{index}
//!   GET            /_plugins/_ism/explain             (list-all — same
//!                  reason, for the "Policy managed indexes" table)

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use xerj_engine::lifecycle::{self, LifecyclePolicy, ManagedIndexState};

use crate::{error::ApiError, state::AppState};

// ─────────────────────────────────────────────────────────────────────────────
// PUT/GET/DELETE /_plugins/_ism/policies/{policy_id}
// ─────────────────────────────────────────────────────────────────────────────

/// Real ISM wraps the policy body as `{"policy": {...}}`; accept that shape
/// but also the bare policy object, since it's an easy, harmless mistake to
/// make by hand (and this parses unambiguously either way).
fn unwrap_policy_body(body: &Value) -> &Value {
    body.get("policy").unwrap_or(body)
}

pub async fn put_ism_policy(
    State(state): State<AppState>,
    Path(policy_id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let policy_value = unwrap_policy_body(&body).clone();
    let policy: LifecyclePolicy = match serde_json::from_value(policy_value) {
        Ok(p) => p,
        Err(e) => {
            let err =
                xerj_common::XerjError::invalid_query(format!("invalid ISM policy body: {e}"));
            return ApiError::new(err).into_response();
        }
    };
    if let Err(reason) = policy.validate() {
        let err = xerj_common::XerjError::invalid_query(format!("invalid ISM policy: {reason}"));
        return ApiError::new(err).into_response();
    }
    state
        .engine
        .put_ism_policy(policy_id.clone(), policy.clone());
    Json(json!({
        "_id": policy_id,
        "_version": 1,
        "_seq_no": 0,
        "_primary_term": 1,
        "policy": policy,
    }))
    .into_response()
}

pub async fn get_ism_policy(
    State(state): State<AppState>,
    Path(policy_id): Path<String>,
) -> impl IntoResponse {
    match state.engine.ism_policies.get(&policy_id) {
        Some(policy) => Json(json!({
            "_id": policy_id,
            "_version": 1,
            "_seq_no": 0,
            "_primary_term": 1,
            "policy": policy.value(),
        }))
        .into_response(),
        None => {
            let e =
                xerj_common::XerjError::index_not_found(format!("policy [{policy_id}] not found"));
            ApiError::new(e).into_response()
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ListParams {
    #[serde(default)]
    pub from: usize,
    #[serde(default = "default_size")]
    pub size: usize,
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub sort_direction: Option<String>,
}
fn default_size() -> usize {
    20
}

/// `GET /_plugins/_ism/policies` — the list/search form OSD's own Index
/// Management UI uses to populate the "State management policies" table
/// (its backend proxies `/api/ism/policies` straight through to this
/// endpoint). Found missing while verifying the real UI: only the
/// single-policy `GET .../policies/{id}` was implemented, so the list page
/// rendered "no existing policies" and a raw Internal Server Error toast
/// even immediately after a successful create.
pub async fn list_ism_policies(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let mut all: Vec<(String, LifecyclePolicy)> = state
        .engine
        .ism_policies
        .iter()
        .map(|e| (e.key().clone(), e.value().clone()))
        .filter(|(id, _)| params.search.is_empty() || id.contains(&params.search))
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    if params.sort_direction.as_deref() == Some("desc") {
        all.reverse();
    }
    let total = all.len();
    let page: Vec<Value> = all
        .into_iter()
        .skip(params.from)
        .take(params.size.max(1))
        .map(|(id, policy)| {
            json!({
                "_id": id,
                "_version": 1,
                "_seq_no": 0,
                "_primary_term": 1,
                "policy": policy,
            })
        })
        .collect();
    Json(json!({ "totalPolicies": total, "policies": page })).into_response()
}

pub async fn delete_ism_policy(
    State(state): State<AppState>,
    Path(policy_id): Path<String>,
) -> impl IntoResponse {
    if state.engine.remove_ism_policy(&policy_id) {
        Json(json!({ "_id": policy_id, "result": "deleted" })).into_response()
    } else {
        let e = xerj_common::XerjError::index_not_found(format!("policy [{policy_id}] not found"));
        ApiError::new(e).into_response()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// POST /_plugins/_ism/add/{index}
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AddPolicyBody {
    pub policy_id: String,
}

pub async fn add_ism_policy(
    State(state): State<AppState>,
    Path(index): Path<String>,
    Json(body): Json<AddPolicyBody>,
) -> impl IntoResponse {
    attach_policy(&state, &index, &body.policy_id).await
}

/// Shared attach implementation: validates the target index and policy both
/// exist, then creates (or resets) that index's managed-index execution
/// cursor at the policy's `default_state`. Used by the native ISM endpoint
/// above and by the ES-shape `index.lifecycle.name` auto-attach in
/// `es_compat` (a single index↔policy association model, per the design —
/// not two).
pub async fn attach_policy(state: &AppState, index: &str, policy_id: &str) -> Response {
    if state.engine.get_index(index).is_err() {
        let e = xerj_common::XerjError::index_not_found(index);
        return ApiError::new(e).into_response();
    }
    let Some(policy) = state
        .engine
        .ism_policies
        .get(policy_id)
        .map(|e| e.value().clone())
    else {
        let e = xerj_common::XerjError::index_not_found(format!("policy [{policy_id}] not found"));
        return ApiError::new(e).into_response();
    };
    let managed = ManagedIndexState::new(
        policy_id.to_string(),
        policy.default_state.clone(),
        lifecycle::now_ms(),
    );
    state
        .engine
        .managed_indices
        .insert(index.to_string(), managed);
    state.engine.persist_managed_indices();

    Json(json!({
        "failures": false,
        "updated_indices": 1,
        "failed_indices": [],
    }))
    .into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// GET /_plugins/_ism/explain/{index}
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /_plugins/_ism/explain` — the list-all form OSD's "Policy managed
/// indexes" table uses (its backend's `/api/ism/managedIndices` proxies
/// straight through to this). Same gap as `list_ism_policies`: only the
/// single-index `.../explain/{index}` was implemented at first.
pub async fn list_managed_indices(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let mut all: Vec<(String, ManagedIndexState)> = state
        .engine
        .managed_indices
        .iter()
        .map(|e| (e.key().clone(), e.value().clone()))
        .filter(|(name, _)| params.search.is_empty() || name.contains(&params.search))
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    if params.sort_direction.as_deref() == Some("desc") {
        all.reverse();
    }
    let total = all.len();
    let mut body = serde_json::Map::new();
    for (name, managed) in all.into_iter().skip(params.from).take(params.size.max(1)) {
        body.insert(name.clone(), lifecycle::explain_json(&name, &managed));
    }
    body.insert("total_managed_indices".to_string(), json!(total));
    Json(Value::Object(body)).into_response()
}

pub async fn explain_ism_index(
    State(state): State<AppState>,
    Path(index): Path<String>,
) -> impl IntoResponse {
    match state.engine.managed_indices.get(&index) {
        Some(managed) => {
            let mut body = serde_json::Map::new();
            body.insert(
                index.clone(),
                lifecycle::explain_json(&index, managed.value()),
            );
            body.insert("total_managed_indices".to_string(), json!(1));
            Json(Value::Object(body)).into_response()
        }
        None => {
            // Real ISM answers 200 with a "not managed" note rather than
            // 404 — `explain` on an unmanaged (or nonexistent) index is a
            // normal, expected call from Dashboards' Managed Indexes view.
            Json(json!({
                index.clone(): { "index": index, "enabled": false },
                "total_managed_indices": 0,
            }))
            .into_response()
        }
    }
}
