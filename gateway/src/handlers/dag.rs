//! DAG routing handlers
//!
//! Provides REST API endpoints for DAG template management and validation.

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, warn};

use crate::state::AppState;

#[cfg(feature = "dag-routing")]
use crate::dag::{
    DAGDefinition, DAGCompiler,
    global_templates,
};

/// List available DAG templates
#[cfg(feature = "dag-routing")]
pub async fn list_templates(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let templates = global_templates();
    let template_names = templates.names();
    let template_list: Vec<TemplateInfo> = template_names
        .into_iter()
        .map(|name: String| {
            let template = templates.get(&name);
            TemplateInfo {
                name: name.clone(),
                version: template.as_ref().map(|t| t.version.clone()).unwrap_or_default(),
                description: template.map(|t| t.name.clone()),
            }
        })
        .collect();

    let count = template_list.len();
    Json(ListTemplatesResponse {
        templates: template_list,
        count,
    })
}

/// List available DAG templates (stub when feature disabled)
#[cfg(not(feature = "dag-routing"))]
pub async fn list_templates(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "DAG routing is not enabled",
            "message": "Build with --features dag-routing to enable DAG support"
        }))
    )
}

/// Validate a DAG definition
#[cfg(feature = "dag-routing")]
pub async fn validate_dag(
    State(_state): State<Arc<AppState>>,
    Json(request): Json<ValidateDAGRequest>,
) -> impl IntoResponse {
    debug!("Validating DAG definition");

    // Parse the DAG definition
    let dag_def: DAGDefinition = match serde_json::from_value(request.dag.clone()) {
        Ok(def) => def,
        Err(e) => {
            warn!(error = %e, "Failed to parse DAG definition");
            return (
                StatusCode::BAD_REQUEST,
                Json(ValidateDAGResponse {
                    valid: false,
                    errors: vec![format!("Failed to parse DAG definition: {}", e)],
                    warnings: vec![],
                    node_count: 0,
                    edge_count: 0,
                })
            );
        }
    };

    let node_count = dag_def.nodes.len();
    let edge_count = dag_def.edges.len();

    // Compile (validates) the DAG
    let compiler = DAGCompiler::new();
    match compiler.compile(dag_def) {
        Ok(_compiled) => {
            (
                StatusCode::OK,
                Json(ValidateDAGResponse {
                    valid: true,
                    errors: vec![],
                    warnings: vec![],
                    node_count,
                    edge_count,
                })
            )
        }
        Err(e) => {
            (
                StatusCode::OK,
                Json(ValidateDAGResponse {
                    valid: false,
                    errors: vec![e.to_string()],
                    warnings: vec![],
                    node_count,
                    edge_count,
                })
            )
        }
    }
}

/// Validate a DAG definition (stub when feature disabled)
#[cfg(not(feature = "dag-routing"))]
pub async fn validate_dag(
    State(_state): State<Arc<AppState>>,
    Json(_request): Json<ValidateDAGRequest>,
) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "DAG routing is not enabled",
            "message": "Build with --features dag-routing to enable DAG support"
        }))
    )
}

/// Get a specific DAG template
#[cfg(feature = "dag-routing")]
pub async fn get_template(
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(template_name): axum::extract::Path<String>,
) -> impl IntoResponse {
    let templates = global_templates();

    match templates.get(&template_name) {
        Some(template) => {
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "name": template_name,
                    "template": template
                }))
            )
        }
        None => {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Template not found",
                    "name": template_name
                }))
            )
        }
    }
}

/// Get a specific DAG template (stub when feature disabled)
#[cfg(not(feature = "dag-routing"))]
pub async fn get_template(
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(_template_name): axum::extract::Path<String>,
) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "DAG routing is not enabled",
            "message": "Build with --features dag-routing to enable DAG support"
        }))
    )
}

// Request/Response types

#[derive(Debug, Serialize)]
pub struct ListTemplatesResponse {
    pub templates: Vec<TemplateInfo>,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct TemplateInfo {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ValidateDAGRequest {
    pub dag: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ValidateDAGResponse {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub node_count: usize,
    pub edge_count: usize,
}
