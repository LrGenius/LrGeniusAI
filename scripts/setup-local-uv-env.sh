#!/usr/bin/env bash
set -euo pipefail

# Creates a local uv virtual environment for backend development/debugging.
# Default profile mirrors desktop builds from CI.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

SERVER_DIR="${ROOT_DIR}/server"
VENV_DIR="${ROOT_DIR}/.venv"
PROFILE="desktop"
PYTHON_VERSION="3.12"

usage() {
  cat <<'EOF'
Usage: scripts/setup-local-uv-env.sh [options]

Options:
  --profile <desktop|docker>   Dependency profile to install (default: desktop)
  --venv-dir <path>            Virtual environment directory (default: .venv)
  --python <version>           Python version for uv venv (default: 3.12)
  -h, --help                   Show this help message

Examples:
  scripts/setup-local-uv-env.sh
  scripts/setup-local-uv-env.sh --profile docker
  scripts/setup-local-uv-env.sh --venv-dir .venv-debug --python 3.12
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      PROFILE="${2:-}"
      shift 2
      ;;
    --venv-dir)
      VENV_DIR="${2:-}"
      shift 2
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

if [[ ! -d "${SERVER_DIR}" ]]; then
  echo "Error: server directory not found at ${SERVER_DIR}" >&2
  exit 1
fi

case "${PROFILE}" in
  desktop)
    REQUIREMENTS_FILE="${SERVER_DIR}/requirements-desktop.txt"
    ;;
  docker)
    REQUIREMENTS_FILE="${SERVER_DIR}/requirements.txt"
    ;;
  *)
    echo "Error: invalid profile '${PROFILE}'. Use 'desktop' or 'docker'." >&2
    exit 1
    ;;
esac

if [[ ! -f "${REQUIREMENTS_FILE}" ]]; then
  echo "Error: requirements file not found: ${REQUIREMENTS_FILE}" >&2
  exit 1
fi

echo "Creating uv environment..."
echo "  profile: ${PROFILE}"
echo "  python:  ${PYTHON_VERSION}"
echo "  venv:    ${VENV_DIR}"
echo "  deps:    ${REQUIREMENTS_FILE}"

uv venv --python "${PYTHON_VERSION}" "${VENV_DIR}"
uv pip install --python "${VENV_DIR}/bin/python" -r "${REQUIREMENTS_FILE}"

echo
echo "Environment ready."
echo "Activate with:"
echo "  source \"${VENV_DIR}/bin/activate\""
echo
echo "Quick backend debug run:"
echo "  PYTHONPATH=\"${ROOT_DIR}/server:\${PYTHONPATH:-}\" \"${VENV_DIR}/bin/python\" \"${ROOT_DIR}/server/src/geniusai_server.py\""
