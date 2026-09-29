// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Hyperlight Authors.

use anyhow::{Context, Result};
use octocrab::Octocrab;
use regex::Regex;
use serde::Deserialize;

const CONFIG_PATH: &str = ".github/hyperlight-bot.yml";

/// Per-repository bot configuration, loaded from `.github/hyperlight-bot.yml`.
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct RepoConfig {
    /// Name of the artifact containing the comment body.
    pub artifact_name: String,
    /// Regex pattern matched against the workflow job name to trigger the bot.
    pub job_filter: String,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            artifact_name: "pr-comment".to_string(),
            job_filter: ".*".to_string(),
        }
    }
}

impl RepoConfig {
    /// Compiles `job_filter` as a regex.
    pub fn job_filter_regex(&self) -> Result<Regex> {
        Regex::new(&self.job_filter)
            .with_context(|| format!("Invalid job_filter regex: {}", self.job_filter))
    }
}

/// Fetches the repo config from `.github/hyperlight-bot.yml` at the given ref.
/// Returns the default config if the file doesn't exist.
pub async fn load(octocrab: &Octocrab, owner: &str, repo: &str, git_ref: &str) -> Result<RepoConfig> {
    let result = octocrab
        .repos(owner, repo)
        .get_content()
        .path(CONFIG_PATH)
        .r#ref(git_ref)
        .send()
        .await;

    let content_items = match result {
        Ok(content) => content,
        Err(octocrab::Error::GitHub { source, .. }) if source.status_code.as_u16() == 404 => {
            tracing::debug!("No {CONFIG_PATH} found in {owner}/{repo}, using defaults");
            return Ok(RepoConfig::default());
        }
        Err(e) => return Err(e).context("Failed to fetch repo config"),
    };

    let file = content_items
        .items
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Empty response for {CONFIG_PATH}"))?;

    let decoded = file
        .decoded_content()
        .ok_or_else(|| anyhow::anyhow!("Could not decode {CONFIG_PATH} content"))?;

    let config: RepoConfig =
        serde_yaml::from_str(&decoded).context("Failed to parse {CONFIG_PATH}")?;

    Ok(config)
}
