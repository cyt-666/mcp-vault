//! Protocol-neutral semantic Admin projections and commands.
//!
//! This module is deliberately an adapter only: Vault selection, canonical
//! reads, qualification, and rule writes remain owned by the shared semantic
//! facade and Vault Core boundary.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, Method, StatusCode},
    response::Response,
    routing::{get, post},
};
use mcp_vault_domain::{Permission, PermissionSet, VaultContext};
use mcp_vault_memory::{
    MemoryOrigin, MemoryPackRequest, SemanticAccess, SemanticActor, SemanticCardKind,
    SemanticCardRequest, SemanticEvidenceAccess, SemanticEvidenceRequest, SemanticExplicitFacade,
    SemanticListCardsRequest, SemanticMutation, SemanticProcessingStatusRequest,
    SemanticPublicFacade, SemanticRememberExplicitRequest, SemanticRuleCommand,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    AdminApiState, AdminPrincipal, RequestId, api_error, api_ok, auth_error, current_vault,
    memory_error, state_error, validate_state_change_origin,
};

const SEMANTIC_ACCESS: &[Permission] = &[
    Permission::ReadMemory,
    Permission::ReadVault,
    Permission::ManageMemory,
];

#[derive(Debug, Deserialize)]
struct SemanticCardQuery {
    card_id: String,
    card_kind: Option<String>,
}

pub(super) fn routes() -> Router<AdminApiState> {
    Router::new()
        .route("/semantic/status", get(get_status))
        .route("/semantic/cards", get(list_cards))
        .route("/semantic/card", get(get_card))
        .route("/semantic/evidence", get(get_evidence))
        .route("/semantic/pack", get(get_pack).post(build_pack))
        .route("/semantic/remember-explicit", post(remember_explicit))
        .route("/semantic/correct", post(correct))
        .route("/semantic/forget", post(forget))
}

fn access() -> SemanticAccess {
    SemanticAccess::new(SEMANTIC_ACCESS.iter().copied().collect::<PermissionSet>())
}

async fn remember_explicit(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    axum::extract::Extension(principal): axum::extract::Extension<AdminPrincipal>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
    Json(input): Json<SemanticRememberExplicitRequest>,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    let (context, core) = match selected(&state, &request_id.0).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match SemanticExplicitFacade::new(state.memory())
        .remember(
            &context,
            &core,
            principal.actor.clone(),
            mcp_vault_domain::SourcePlane::Admin,
            MemoryOrigin::ExplicitAdmin,
            input,
        )
        .await
    {
        Ok(result) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.semantic.explicit_remembered",
                    Some("explicit_memory"),
                    Some(&result.memory.memory_id),
                    json!({
                        "outcome": result.outcome,
                        "revision": result.memory.revision,
                        "embedding_binding_present": result.memory.embedding_binding_present,
                    }),
                )
                .await;
            api_ok(StatusCode::OK, json!({"explicit": result}), request_id.0)
        }
        Err(error) => memory_error(error, request_id.0),
    }
}

async fn selected(
    state: &AdminApiState,
    request_id: &str,
) -> Result<(VaultContext, mcp_vault_core::VaultCore), Response> {
    let vault = current_vault(state, request_id).await?;
    let context = vault.context().map_err(|_| {
        state_error(
            mcp_vault_state::StateError::InvalidInput("Vault context is invalid"),
            request_id.to_owned(),
        )
    })?;
    let core = state
        .core_for_vault(&vault)
        .map_err(|error| state_error(error, request_id.to_owned()))?;
    Ok((context, core))
}

async fn list_cards(
    State(state): State<AdminApiState>,
    Query(input): Query<SemanticListCardsRequest>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
) -> Response {
    let limit = input.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The semantic card limit is invalid.",
            None,
            request_id.0,
        );
    }
    let (context, core) = match selected(&state, &request_id.0).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match SemanticPublicFacade::new(state.state.clone())
        .list_cards(&context, &core, &access(), limit)
        .await
    {
        Ok((cards, composed_cards)) => api_ok(
            StatusCode::OK,
            json!({"cards": cards, "composed_cards": composed_cards}),
            request_id.0,
        ),
        Err(error) => memory_error(error, request_id.0),
    }
}

async fn get_card(
    State(state): State<AdminApiState>,
    Query(query): Query<SemanticCardQuery>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
) -> Response {
    let card_kind = match query.card_kind.as_deref() {
        None => None,
        Some("card") => Some(SemanticCardKind::Card),
        Some("composed_card") => Some(SemanticCardKind::ComposedCard),
        Some(_) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation_failed",
                "The semantic card kind is invalid.",
                None,
                request_id.0,
            );
        }
    };
    let input = SemanticCardRequest {
        card_id: query.card_id,
        card_kind,
    };
    let (context, core) = match selected(&state, &request_id.0).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let facade = SemanticPublicFacade::new(state.state.clone());
    let result = match input.card_kind {
        None | Some(SemanticCardKind::Card) => facade
            .get_card(&context, &core, &access(), &input.card_id)
            .await
            .map(|card| card.map(|value| json!({"card": value}))),
        Some(SemanticCardKind::ComposedCard) => facade
            .get_composed_card(&context, &core, &access(), &input.card_id)
            .await
            .map(|card| card.map(|value| json!({"composed_card": value}))),
    };
    match result {
        Ok(Some(value)) => api_ok(StatusCode::OK, value, request_id.0),
        Ok(None) => api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "The semantic card was not found.",
            None,
            request_id.0,
        ),
        Err(error) => memory_error(error, request_id.0),
    }
}

async fn get_evidence(
    State(state): State<AdminApiState>,
    Query(input): Query<SemanticEvidenceRequest>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
) -> Response {
    let (context, core) = match selected(&state, &request_id.0).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let access_input = SemanticEvidenceAccess {
        source_id: input.source_id,
        source_revision_id: input.source_revision_id,
        parent_ref: input.parent_ref,
    };
    match SemanticPublicFacade::new(state.state.clone())
        .read_evidence(
            &context,
            &core,
            &access(),
            &access_input,
            &input.evidence_ref_id,
        )
        .await
    {
        Ok(Some(evidence)) => api_ok(StatusCode::OK, json!({"evidence": evidence}), request_id.0),
        Ok(None) => api_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "The semantic evidence was not found.",
            None,
            request_id.0,
        ),
        Err(error) => memory_error(error, request_id.0),
    }
}

async fn get_pack(
    State(state): State<AdminApiState>,
    Query(input): Query<MemoryPackRequest>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
) -> Response {
    build_pack_inner(state, input, request_id).await
}

async fn build_pack(
    State(state): State<AdminApiState>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
    Json(input): Json<MemoryPackRequest>,
) -> Response {
    build_pack_inner(state, input, request_id).await
}

async fn build_pack_inner(
    state: AdminApiState,
    input: MemoryPackRequest,
    request_id: RequestId,
) -> Response {
    let (context, core) = match selected(&state, &request_id.0).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match SemanticPublicFacade::new(state.state.clone())
        .build_pack(&context, &core, &access(), &input)
        .await
    {
        Ok(pack) => api_ok(StatusCode::OK, pack, request_id.0),
        Err(error) => memory_error(error, request_id.0),
    }
}

async fn get_status(
    State(state): State<AdminApiState>,
    Query(input): Query<SemanticProcessingStatusRequest>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
) -> Response {
    let (context, _core) = match selected(&state, &request_id.0).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    match SemanticPublicFacade::new(state.state.clone())
        .processing_status(&context, &access(), input.limit.unwrap_or(50))
        .await
    {
        Ok(status) => api_ok(StatusCode::OK, status, request_id.0),
        Err(error) => memory_error(error, request_id.0),
    }
}

async fn correct(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    axum::extract::Extension(principal): axum::extract::Extension<AdminPrincipal>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
    Json(input): Json<SemanticRuleCommand>,
) -> Response {
    apply_rule(
        state,
        headers,
        principal,
        request_id,
        input,
        SemanticMutation::Correction,
    )
    .await
}

async fn forget(
    State(state): State<AdminApiState>,
    headers: HeaderMap,
    axum::extract::Extension(principal): axum::extract::Extension<AdminPrincipal>,
    axum::extract::Extension(request_id): axum::extract::Extension<RequestId>,
    Json(input): Json<SemanticRuleCommand>,
) -> Response {
    apply_rule(
        state,
        headers,
        principal,
        request_id,
        input,
        SemanticMutation::ForgetCurrent,
    )
    .await
}

async fn apply_rule(
    state: AdminApiState,
    headers: HeaderMap,
    principal: AdminPrincipal,
    request_id: RequestId,
    input: SemanticRuleCommand,
    expected_mutation: SemanticMutation,
) -> Response {
    if let Err(error) = validate_state_change_origin(&state, &headers, &Method::POST) {
        return auth_error(error, request_id.0);
    }
    if input.mutation != expected_mutation
        && !(matches!(expected_mutation, SemanticMutation::ForgetCurrent)
            && matches!(
                input.mutation,
                SemanticMutation::SuppressRead | SemanticMutation::SuppressRegeneration
            ))
    {
        return api_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation_failed",
            "The semantic mutation does not match this endpoint.",
            None,
            request_id.0,
        );
    }
    let (context, _core) = match selected(&state, &request_id.0).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let actor = SemanticActor::trusted(format!("admin:{:?}", principal.actor));
    match SemanticPublicFacade::new(state.state.clone())
        .apply_rule(&context, &access(), &actor, &input)
        .await
    {
        Ok(rule) => {
            state
                .append_admin_audit(
                    Some(&context),
                    &request_id.0,
                    &principal.actor,
                    "admin.semantic.rule_applied",
                    Some("semantic_rule"),
                    Some(&rule.id),
                    json!({
                        "action": rule.action,
                        "target_ref": rule.target_ref,
                        "rules_revision": rule.rules_revision,
                    }),
                )
                .await;
            api_ok(StatusCode::OK, rule, request_id.0)
        }
        Err(error) => memory_error(error, request_id.0),
    }
}
