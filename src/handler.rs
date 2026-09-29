// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Hyperlight Authors.

use anyhow::{Context, Result};

use crate::auth;
use crate::config::Config;
use crate::repo_config;

/// Lists the PR's issue comments, flagging the ones authored by the bot itself
/// (`viewerDidAuthor`) and the ones already hidden (`isMinimized`). The body is
/// needed to recognise which artifact a comment was posted for.
const LIST_COMMENTS_QUERY: &str = r#"
query ListComments($owner: String!, $repo: String!, $number: Int!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      comments(first: 100, after: $cursor) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          body
          isMinimized
          viewerDidAuthor
        }
      }
    }
  }
}
"#;

const MINIMIZE_COMMENT_MUTATION: &str = r#"
mutation MinimizeComment($subjectId: ID!) {
  minimizeComment(input: { classifier: OUTDATED, subjectId: $subjectId }) {
    minimizedComment {
      isMinimized
    }
  }
}
"#;

/// Every job in a workflow run reports completion separately, and with the
/// default `job_filter` of `.*` each one matches. They all resolve to the same
/// artifact, so without a guard the same content gets posted once per job.
///
/// Each comment therefore records which artifact produced it, and an artifact
/// that already has a comment is never posted again.
fn artifact_marker(artifact_id: u64) -> String {
    format!("<!-- hyperlight-gh-bot:artifact={artifact_id} -->")
}

/// Serialises the check-and-post section. Jobs can finish simultaneously, and
/// the marker alone cannot stop two handlers that both look before either
/// writes. The bot runs as a single replica, so an in-process lock closes that
/// window; the marker still covers restarts and redeployments.
static POST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A PR comment as needed for deduplication and hiding.
struct BotComment {
    node_id: String,
    body: String,
    is_minimized: bool,
}

/// Downloads the comment artifact from a workflow run and posts it to the associated PR.
/// The artifact is expected to be a zip containing a single text file with the comment body.
pub async fn try_post_benchmark_comment(
    config: &Config,
    owner: &str,
    repo: &str,
    run_id: u64,
    head_sha: &str,
    installation_id: u64,
    job_name: &str,
) -> Result<()> {
    let octocrab = auth::octocrab_for_installation(
        config.app_id,
        &config.private_key,
        installation_id,
    )
    .await?;

    // Load per-repo config (or defaults if file not found)
    let repo_config = repo_config::load(&octocrab, owner, repo, head_sha).await?;

    // Check if this job matches the configured filter
    let job_regex = repo_config.job_filter_regex()?;
    if !job_regex.is_match(job_name) {
        tracing::debug!(
            "Job '{job_name}' does not match filter '{}', skipping",
            repo_config.job_filter
        );
        return Ok(());
    }

    tracing::info!("Processing job '{job_name}' for SHA {head_sha}");

    // Find the PR associated with this head SHA
    let pr_number = find_pr_for_sha(&octocrab, owner, repo, head_sha)
        .await?
        .ok_or_else(|| anyhow::anyhow!("No open PR found for SHA {head_sha}"))?;

    tracing::info!("Found PR #{pr_number} for SHA {head_sha}");

    // Find the comment artifact
    let artifact_id = find_artifact(&octocrab, owner, repo, run_id, &repo_config.artifact_name)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Artifact '{}' not found for run {run_id}",
                repo_config.artifact_name
            )
        })?;

    // Download the artifact content — this is the comment body
    let body = download_artifact_text(&octocrab, owner, repo, artifact_id).await?;

    post_pr_comment(&octocrab, owner, repo, pr_number, &body, artifact_id).await?;

    Ok(())
}

/// Finds an open PR whose head SHA matches the given SHA.
async fn find_pr_for_sha(
    octocrab: &octocrab::Octocrab,
    owner: &str,
    repo: &str,
    head_sha: &str,
) -> Result<Option<u64>> {
    let pulls = octocrab.pulls(owner, repo);
    let prs = pulls
        .list()
        .state(octocrab::params::State::Open)
        .send()
        .await
        .context("Failed to list pull requests")?;

    for pr in prs.items {
        if pr.head.sha == head_sha {
            return Ok(Some(pr.number));
        }
    }
    Ok(None)
}

/// Finds an artifact by exact name in a workflow run.
async fn find_artifact(
    octocrab: &octocrab::Octocrab,
    owner: &str,
    repo: &str,
    run_id: u64,
    artifact_name: &str,
) -> Result<Option<u64>> {
    let response: serde_json::Value = octocrab
        .get(
            format!("/repos/{owner}/{repo}/actions/runs/{run_id}/artifacts"),
            None::<&()>,
        )
        .await
        .context("Failed to list artifacts")?;

    let artifacts = response["artifacts"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    Ok(artifacts.iter().find_map(|a| {
        let name = a["name"].as_str()?;
        if name == artifact_name {
            a["id"].as_u64()
        } else {
            None
        }
    }))
}

/// Downloads an artifact (expects a zip containing a single text file)
/// and returns its content as a string.
async fn download_artifact_text(
    octocrab: &octocrab::Octocrab,
    owner: &str,
    repo: &str,
    artifact_id: u64,
) -> Result<String> {
    let route = format!("/repos/{owner}/{repo}/actions/artifacts/{artifact_id}/zip");

    let response = octocrab
        ._get(route)
        .await
        .context("Failed to download artifact")?;

    use http_body_util::BodyExt;
    let bytes = response
        .into_body()
        .collect()
        .await
        .context("Failed to read artifact response body")?
        .to_bytes();

    // The response is a zip file — extract the first file
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).context("Failed to read artifact zip")?;

    let mut file = archive.by_index(0).context("Artifact zip is empty")?;
    let mut content = String::new();
    std::io::Read::read_to_string(&mut file, &mut content)?;

    Ok(content)
}

/// Creates a comment for this artifact unless one already exists, then hides the
/// bot's earlier comments as outdated.
async fn post_pr_comment(
    octocrab: &octocrab::Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
    body: &str,
    artifact_id: u64,
) -> Result<()> {
    let marker = artifact_marker(artifact_id);

    // Held across the whole check-and-post so two jobs finishing together
    // cannot both decide the artifact is unposted.
    let guard = POST_LOCK.lock().await;

    // Listing before posting means the new comment cannot appear in the set,
    // so there is no need to filter it back out afterwards.
    let existing = list_pr_comments(octocrab, owner, repo, pr_number).await?;

    if existing.iter().any(|c| c.body.contains(&marker)) {
        tracing::info!("Artifact {artifact_id} already has a comment on PR #{pr_number}, skipping");
        return Ok(());
    }

    octocrab
        .issues(owner, repo)
        .create_comment(pr_number, format!("{body}\n\n{marker}"))
        .await
        .context("Failed to create comment")?;

    tracing::info!("Posted comment for artifact {artifact_id} on PR #{pr_number}");

    // Past the point another handler could duplicate this artifact, so let
    // other pull requests proceed instead of waiting on the hiding below.
    drop(guard);

    // The comment is already published, so a cleanup failure must not fail the
    // whole run.
    if let Err(e) = hide_comments(octocrab, &existing, pr_number).await {
        tracing::warn!("Failed to hide previous bot comments on PR #{pr_number}: {e:#}");
    }

    Ok(())
}

/// Fetches every comment on the PR, following pagination.
async fn list_pr_comments(
    octocrab: &octocrab::Octocrab,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Result<Vec<BotComment>> {
    let mut cursor: Option<String> = None;
    let mut out = Vec::new();

    loop {
        let response: serde_json::Value = octocrab
            .graphql(&serde_json::json!({
                "query": LIST_COMMENTS_QUERY,
                "variables": {
                    "owner": owner,
                    "repo": repo,
                    "number": pr_number,
                    "cursor": cursor,
                },
            }))
            .await
            .context("Failed to list PR comments")?;
        check_graphql_errors(&response)?;

        let comments = &response["data"]["repository"]["pullRequest"]["comments"];

        for node in comments["nodes"].as_array().into_iter().flatten() {
            let Some(id) = node["id"].as_str() else {
                continue;
            };
            if node["viewerDidAuthor"] != true {
                continue;
            }
            out.push(BotComment {
                node_id: id.to_owned(),
                body: node["body"].as_str().unwrap_or_default().to_owned(),
                is_minimized: node["isMinimized"] == true,
            });
        }

        if comments["pageInfo"]["hasNextPage"] != true {
            break;
        }
        cursor = comments["pageInfo"]["endCursor"]
            .as_str()
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }

    Ok(out)
}

/// Minimizes each of the given comments that is not already hidden.
async fn hide_comments(
    octocrab: &octocrab::Octocrab,
    comments: &[BotComment],
    pr_number: u64,
) -> Result<()> {
    let stale: Vec<&BotComment> = comments.iter().filter(|c| !c.is_minimized).collect();

    let mut hidden = 0;
    for c in &stale {
        match minimize_comment_as_outdated(octocrab, &c.node_id).await {
            Ok(()) => hidden += 1,
            Err(e) => tracing::warn!("Failed to hide previous bot comment {}: {e:#}", c.node_id),
        }
    }

    tracing::info!(
        "Hid {hidden} of {} previous bot comment(s) on PR #{pr_number}",
        stale.len()
    );

    Ok(())
}

async fn minimize_comment_as_outdated(
    octocrab: &octocrab::Octocrab,
    subject_id: &str,
) -> Result<()> {
    let response: serde_json::Value = octocrab
        .graphql(&serde_json::json!({
            "query": MINIMIZE_COMMENT_MUTATION,
            "variables": {
                "subjectId": subject_id,
            },
        }))
        .await
        .context("Failed to minimize comment")?;
    check_graphql_errors(&response)?;

    if response["data"]["minimizeComment"]["minimizedComment"]["isMinimized"] != true {
        anyhow::bail!("GitHub reported the comment as not minimized");
    }

    Ok(())
}

/// GraphQL reports failures in the `errors` field of an otherwise successful response.
fn check_graphql_errors(response: &serde_json::Value) -> Result<()> {
    match response["errors"].as_array() {
        Some(errors) if !errors.is_empty() => {
            anyhow::bail!("GitHub GraphQL API returned errors: {}", response["errors"])
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        artifact_marker, check_graphql_errors, LIST_COMMENTS_QUERY, MINIMIZE_COMMENT_MUTATION,
    };

    #[test]
    fn minimize_mutation_uses_outdated_classifier() {
        assert!(MINIMIZE_COMMENT_MUTATION.contains("classifier: OUTDATED"));
        assert!(MINIMIZE_COMMENT_MUTATION.contains("subjectId: $subjectId"));
    }

    #[test]
    fn list_query_requests_the_fields_dedup_and_hiding_need() {
        assert!(LIST_COMMENTS_QUERY.contains("viewerDidAuthor"));
        assert!(LIST_COMMENTS_QUERY.contains("isMinimized"));
        assert!(LIST_COMMENTS_QUERY.contains("hasNextPage"));
        // The body carries the artifact marker.
        assert!(LIST_COMMENTS_QUERY.contains("body"));
    }

    #[test]
    fn artifact_marker_is_hidden_and_distinguishes_artifacts() {
        let m = artifact_marker(4242);
        assert!(
            m.starts_with("<!--") && m.ends_with("-->"),
            "must be an HTML comment"
        );
        assert!(m.contains("4242"));
        assert_ne!(m, artifact_marker(4243));
    }

    #[test]
    fn a_body_is_matched_only_by_its_own_artifact_marker() {
        let body = format!("## Benchmark Results\n\n{}", artifact_marker(100));
        assert!(body.contains(&artifact_marker(100)));
        assert!(!body.contains(&artifact_marker(101)));
        // A marker must not match a longer id that merely starts the same way.
        assert!(!body.contains(&artifact_marker(1000)));
        // A comment predating this change has no marker at all.
        assert!(!"## Benchmark Results".contains(&artifact_marker(100)));
    }

    #[test]
    fn graphql_errors_are_detected() {
        let ok = serde_json::json!({ "data": { "minimizeComment": {} } });
        assert!(check_graphql_errors(&ok).is_ok());

        let empty_errors = serde_json::json!({ "data": {}, "errors": [] });
        assert!(check_graphql_errors(&empty_errors).is_ok());

        let failed = serde_json::json!({
            "data": null,
            "errors": [{ "message": "Resource not accessible by integration" }],
        });
        let err = check_graphql_errors(&failed).unwrap_err().to_string();
        assert!(err.contains("Resource not accessible by integration"));
    }
}
