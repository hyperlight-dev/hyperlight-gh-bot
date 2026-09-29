// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Hyperlight Authors.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::routing::post;
use tracing_subscriber::EnvFilter;

mod auth;
mod config;
mod handler;
mod repo_config;
mod webhook;

use config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = Config::from_env()?;
    let state = Arc::new(config);

    let app = Router::new()
        .route("/webhook", post(webhook::handle))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("Listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
