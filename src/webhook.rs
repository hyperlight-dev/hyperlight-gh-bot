// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Hyperlight Authors.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::config::Config;
use crate::handler;

/// Webhook event types we care about.
#[derive(serde::Deserialize)]
struct WorkflowJobEvent {
    action: String,
    workflow_job: WorkflowJob,
    repository: Repository,
    installation: Installation,
}

#[derive(serde::Deserialize)]
struct WorkflowJob {
    name: String,
    head_sha: String,
    run_id: u64,
    conclusion: Option<String>,
}

#[derive(serde::Deserialize)]
struct Repository {
    owner: RepoOwner,
    name: String,
}

#[derive(serde::Deserialize)]
struct RepoOwner {
    login: String,
}

#[derive(serde::Deserialize)]
struct Installation {
    id: u64,
}

/// Main webhook handler — verifies the signature and dispatches events.
pub async fn handle(
    State(config): State<Arc<Config>>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Verify webhook signature
    if let Err(e) = verify_signature(&headers, &body, &config.webhook_secret) {
        tracing::warn!("Webhook signature verification failed: {e}");
        return StatusCode::UNAUTHORIZED;
    }

    // Route by event type
    let event_type = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    match event_type {
        "workflow_job" => handle_workflow_job(&config, &body).await,
        "ping" => {
            tracing::info!("Received ping event");
            StatusCode::OK
        }
        _ => {
            tracing::debug!("Ignoring event: {event_type}");
            StatusCode::OK
        }
    }
}

/// Handles a `workflow_job` event — triggers comment posting when a job completes.
async fn handle_workflow_job(config: &Config, body: &[u8]) -> StatusCode {
    let event: WorkflowJobEvent = match serde_json::from_slice(body) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Failed to parse workflow_job event: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    if event.action != "completed" {
        return StatusCode::OK;
    }

    tracing::info!(
        "Workflow job '{}' completed (conclusion: {:?}) for SHA {}",
        event.workflow_job.name,
        event.workflow_job.conclusion,
        event.workflow_job.head_sha,
    );

    let owner = event.repository.owner.login.clone();
    let repo = event.repository.name.clone();
    let run_id = event.workflow_job.run_id;
    let head_sha = event.workflow_job.head_sha.clone();
    let installation_id = event.installation.id;
    let job_name = event.workflow_job.name.clone();

    // Spawn in background so we respond to GitHub immediately
    let config = config.clone();
    tokio::spawn(async move {
        if let Err(e) =
            handler::try_post_benchmark_comment(&config, &owner, &repo, run_id, &head_sha, installation_id, &job_name).await
        {
            tracing::error!("Failed to post benchmark comment: {e:#}");
        }
    });

    StatusCode::OK
}

/// Verifies the HMAC-SHA256 webhook signature.
fn verify_signature(headers: &HeaderMap, body: &[u8], secret: &str) -> anyhow::Result<()> {
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("sha256="))
        .ok_or_else(|| anyhow::anyhow!("Missing X-Hub-Signature-256 header"))?;

    let expected = hex::decode(signature)?;

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())?;
    mac.update(body);
    mac.verify_slice(&expected)
        .map_err(|_| anyhow::anyhow!("Signature mismatch"))
}
