// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Hyperlight Authors.

use anyhow::{Context, Result};

/// Bot configuration loaded from environment variables.
#[derive(Clone)]
pub struct Config {
    /// GitHub App ID.
    pub app_id: u64,
    /// GitHub App private key (PEM format).
    pub private_key: String,
    /// Webhook secret for signature verification.
    pub webhook_secret: String,
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            app_id: std::env::var("GITHUB_APP_ID")
                .context("GITHUB_APP_ID not set")?
                .parse()
                .context("GITHUB_APP_ID must be a number")?,
            private_key: std::env::var("GITHUB_APP_KEY")
                .context("GITHUB_APP_KEY not set")?,
            webhook_secret: std::env::var("GITHUB_WEBHOOK_SECRET")
                .context("GITHUB_WEBHOOK_SECRET not set")?,
        })
    }
}
