// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Hyperlight Authors.

use anyhow::{Context, Result};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use octocrab::Octocrab;
use serde::Serialize;

/// JWT claims for GitHub App authentication.
#[derive(Serialize)]
struct Claims {
    iat: u64,
    exp: u64,
    iss: String,
}

/// Creates an authenticated Octocrab client using a GitHub App installation token.
pub async fn octocrab_for_installation(
    app_id: u64,
    private_key: &str,
    installation_id: u64,
) -> Result<Octocrab> {
    let jwt = generate_jwt(app_id, private_key)?;

    let octocrab = Octocrab::builder()
        .personal_token(jwt)
        .build()
        .context("Failed to build Octocrab client")?;

    // Exchange the JWT for an installation access token
    let token: octocrab::models::InstallationToken = octocrab
        .post(
            format!("/app/installations/{installation_id}/access_tokens"),
            None::<&()>,
        )
        .await
        .context("Failed to create installation token")?;

    Octocrab::builder()
        .personal_token(token.token)
        .build()
        .context("Failed to build authenticated Octocrab client")
}

/// Generates a JWT signed with the App's private key, valid for 10 minutes.
fn generate_jwt(app_id: u64, private_key: &str) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    let claims = Claims {
        iat: now - 60, // 1 minute in the past to account for clock drift
        exp: now + 600, // 10 minutes
        iss: app_id.to_string(),
    };

    let key = EncodingKey::from_rsa_pem(private_key.as_bytes())
        .context("Invalid RSA private key")?;

    jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key)
        .context("Failed to encode JWT")
}
