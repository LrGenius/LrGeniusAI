#!/usr/bin/env bash
set -euo pipefail

# Sets up the backend's local development environment via `uv sync`.
#
# Reads the locked dependency set from server/pyproject.toml + server/uv.lock
# and creates server/.venv/ as the project's virtual environment.
#
# This is a thin convenience wrapper. The equivalent direct command is:
#   cd server && uv sync --locked

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
SERVER_DIR="${ROOT_DIR}/server"

INCLUDE_DEV=1
PYTHON_VERSION=""

usage() {
  cat <<'EOF'
Usage: scripts/setup-local-uv-env.sh [options]

Options:
  --no-dev             Omit the dev dependency group (matches the production Dockerfile)
  --python <version>   Pin the venv to a specific Python version (default: from .python-version)
  -h, --help           Show this help message

Examples:
  scripts/setup-local-uv-env.sh                # full dev environment (runtime + dev deps)
  scripts/setup-local-uv-env.sh --no-dev       # runtime-only, mirrors production
  scripts/setup-local-uv-env.sh --python 3.12  # override the project's pinned Python
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-dev)
      INCLUDE_DEV=0
      shift
      ;;
    --python)
      PYTHON_VERSION="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if ! command -v uv >/dev/null 2>&1; then
  echo "Error: uv is not installed or not in PATH." >&2
  echo "Install instructions: https://docs.astral.sh/uv/getting-started/installation/" >&2
  exit 1
fi

if [[ ! -f "${SERVER_DIR}/pyproject.toml" ]]; then
  echo "Error: ${SERVER_DIR}/pyproject.toml not found." >&2
  exit 1
fi

UV_ARGS=(sync --locked)
if [[ ${INCLUDE_DEV} -eq 0 ]]; then
  UV_ARGS+=(--no-dev)
fi
if [[ -n "${PYTHON_VERSION}" ]]; then
  UV_ARGS+=(--python "${PYTHON_VERSION}")
fi

echo "Running 'uv ${UV_ARGS[*]}' in ${SERVER_DIR}..."
( cd "${SERVER_DIR}" && uv "${UV_ARGS[@]}" )

echo
echo "Environment ready at: ${SERVER_DIR}/.venv"
echo
echo "Next steps:"
echo "  cd \"${SERVER_DIR}\" && uv run python src/geniusai_server.py --db-path /tmp/lrgeniusai-data"
echo "  cd \"${SERVER_DIR}\" && uv run pytest test/"
