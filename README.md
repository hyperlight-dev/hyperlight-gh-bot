# hyperlight-gh-bot

A GitHub App webhook server that posts workflow artifact content as PR comments.

## How it works

1. A CI workflow runs and uploads an artifact containing the comment body (default name: `pr-comment`).
2. GitHub sends a `workflow_job` webhook event when the job completes.
3. The bot downloads the artifact and posts a new comment on the associated PR.
4. Comments the bot previously posted on that PR are hidden as `outdated`.

Per-repository behavior is configured via `.github/hyperlight-bot.yml`:

```yaml
# Name of the artifact containing the comment body (default: "pr-comment")
artifact_name: "pr-comment"
# Regex matched against the job name to filter which jobs trigger the bot (default: ".*")
job_filter: ".*"
```

## Prerequisites

- A registered [GitHub App](https://docs.github.com/en/apps/creating-github-apps) with:
  - **Webhook URL** pointing to this server's `/webhook` endpoint
  - **Webhook secret** configured
  - **Permissions**: `actions: read`, `pull_requests: write`
  - **Events**: `workflow_job`
- The App installed on the target repository
- Rust toolchain (for building locally)

## Configuration

The bot is configured via environment variables:

| Variable | Description |
|----------|-------------|
| `GITHUB_APP_ID` | The numeric App ID |
| `GITHUB_APP_KEY` | The App's private key (PEM format, including `-----BEGIN...` markers) |
| `GITHUB_WEBHOOK_SECRET` | The webhook secret configured in the App settings |

The deployed values are stored in Azure and are the source of truth — the
`github-app-key` and `github-webhook-secret` secrets in the
`hyperlight-gh-bot-kv` Key Vault. Keeping local copies of the private key or
webhook secret is unnecessary; fetch them on demand as shown below.

## Running locally

Requires `az login` with access to the Key Vault:

```bash
RESOURCE_GROUP="hyperlight-gh-bot-rg"
APP_NAME="hyperlight-gh-bot"
KEY_VAULT="hyperlight-gh-bot-kv"

export GITHUB_APP_ID="$(az containerapp show \
  --resource-group $RESOURCE_GROUP --name $APP_NAME \
  --query "properties.template.containers[0].env[?name=='GITHUB_APP_ID'].value | [0]" -o tsv)"
export GITHUB_APP_KEY="$(az keyvault secret show \
  --vault-name $KEY_VAULT --name github-app-key --query value -o tsv)"
export GITHUB_WEBHOOK_SECRET="$(az keyvault secret show \
  --vault-name $KEY_VAULT --name github-webhook-secret --query value -o tsv)"

cargo run
```

The server listens on port 8080. For local development, use a tunnel (e.g. `ngrok http 8080`) to expose it to GitHub.

## Building the Docker image

Reusing the environment variables exported above:

```bash
docker build -t hyperlight-gh-bot .
docker run -p 8080:8080 \
  -e GITHUB_APP_ID="$GITHUB_APP_ID" \
  -e GITHUB_APP_KEY="$GITHUB_APP_KEY" \
  -e GITHUB_WEBHOOK_SECRET="$GITHUB_WEBHOOK_SECRET" \
  hyperlight-gh-bot
```

## Deployment

See [DEPLOY.md](DEPLOY.md) for full GitHub App setup and Azure Container Apps deployment instructions.

## Code checks

Every Rust source file carries an Apache-2.0 license header, matching the
convention in [hyperlight](https://github.com/hyperlight-dev/hyperlight):

```rust
// SPDX-License-Identifier: Apache-2.0
// Copyright <year> The Hyperlight Authors.
```

This is enforced in CI by the **Code checks** workflow, and can be run locally:

```bash
./dev/check-license-headers.sh
```

## Logging

Set the `RUST_LOG` environment variable to control log verbosity:

```bash
RUST_LOG=info cargo run        # default recommended level
RUST_LOG=debug cargo run       # verbose for troubleshooting
```
