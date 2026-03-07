#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${GITHUB_REPOSITORY:-}" ]]; then
  echo "GITHUB_REPOSITORY is required (example: owner/repo)"
  exit 1
fi

if [[ -z "${GITHUB_TOKEN:-}" ]]; then
  echo "GITHUB_TOKEN is required"
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WIKI_SRC_DIR="${ROOT_DIR}/docs/wiki"

if [[ ! -d "${WIKI_SRC_DIR}" ]]; then
  echo "Wiki source directory not found: ${WIKI_SRC_DIR}"
  exit 1
fi

# Ensure generated wiki pages from README files are up to date.
if [[ -x "${ROOT_DIR}/scripts/build-wiki-pages.sh" ]]; then
  "${ROOT_DIR}/scripts/build-wiki-pages.sh"
else
  bash "${ROOT_DIR}/scripts/build-wiki-pages.sh"
fi

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

WIKI_REPO_URL="https://x-access-token:${GITHUB_TOKEN}@github.com/${GITHUB_REPOSITORY}.wiki.git"

echo "Cloning wiki repo..."
git clone "${WIKI_REPO_URL}" "${TMP_DIR}/wiki"

echo "Syncing docs/wiki -> wiki root..."
rsync -a --delete --exclude ".git" "${WIKI_SRC_DIR}/" "${TMP_DIR}/wiki/"

cd "${TMP_DIR}/wiki"

if [[ -z "$(git status --porcelain)" ]]; then
  echo "No wiki changes to publish."
  exit 0
fi

git config user.name "github-actions[bot]"
git config user.email "github-actions[bot]@users.noreply.github.com"

git add .
git commit -m "docs: sync wiki from docs/wiki"
git push origin master

echo "Wiki publish complete."
