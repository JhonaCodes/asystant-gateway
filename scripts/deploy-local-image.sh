#!/usr/bin/env bash
# Build locally and publish; Dokploy pulls the image without compiling Rust.
set -euo pipefail

IMAGE_REPOSITORY="ghcr.io/jhonacodes/asystant-gateway"
SOURCE_REPOSITORY="https://github.com/JhonaCodes/asystant-gateway"
TARGET_PLATFORM="${TARGET_PLATFORM:-linux/amd64}"

if [[ "${1:-}" == "--help" ]]; then
  cat <<'HELP'
Usage: ./scripts/deploy-local-image.sh

Build and publish the current synchronized main commit to GHCR as:
  ghcr.io/jhonacodes/asystant-gateway:<full-commit-sha>
  ghcr.io/jhonacodes/asystant-gateway:prod

Requires Docker with Buildx and gh authenticated with write:packages access.
Default target: linux/amd64. Override TARGET_PLATFORM for another server platform.
This script does not restart Dokploy or pass runtime secrets into the build.
HELP
  exit 0
fi
if [[ $# -ne 0 ]]; then
  echo 'Unexpected argument. Use --help for usage.' >&2
  exit 1
fi

for command_name in docker gh git; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "Required command not found: ${command_name}" >&2
    exit 1
  fi
done

# Resolve this checkout even when invoked from a different working directory.
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(git -C "$script_dir" rev-parse --show-toplevel)
cd "$repo_root"

current_branch=$(git branch --show-current)
if [[ "$current_branch" != "main" ]]; then
  echo "Publishing requires main; current branch: ${current_branch}" >&2
  exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
  echo 'Publishing requires a clean worktree. Commit and push changes first.' >&2
  exit 1
fi

git fetch origin main
commit_sha=$(git rev-parse HEAD)
remote_sha=$(git rev-parse origin/main)
if [[ "$commit_sha" != "$remote_sha" ]]; then
  echo 'Local main must match origin/main before publishing.' >&2
  exit 1
fi

docker info >/dev/null
docker buildx version >/dev/null
gh auth status >/dev/null
github_actor=$(gh api user --jq .login)
echo "Logging in to GHCR as ${github_actor}"
gh auth token | docker login ghcr.io --username "$github_actor" --password-stdin

echo "Building ${IMAGE_REPOSITORY}:${commit_sha} for ${TARGET_PLATFORM}"
docker buildx build \
  --platform "$TARGET_PLATFORM" \
  --file Dockerfile \
  --label "org.opencontainers.image.source=${SOURCE_REPOSITORY}" \
  --label "org.opencontainers.image.revision=${commit_sha}" \
  --label 'org.opencontainers.image.licenses=MIT' \
  --tag "${IMAGE_REPOSITORY}:${commit_sha}" \
  --tag "${IMAGE_REPOSITORY}:prod" \
  --push \
  .

# Resolve registry metadata after publication; do not report only a local build.
docker buildx imagetools inspect "${IMAGE_REPOSITORY}:${commit_sha}"
echo "Published: ${IMAGE_REPOSITORY}:prod"
echo "Immutable rollback reference: ${IMAGE_REPOSITORY}:${commit_sha}"
echo 'In Dokploy select a Docker image source, retain /data and runtime environment, then deploy.'
