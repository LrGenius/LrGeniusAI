#!/bin/bash
set -euo pipefail

# ensure no linting errors
echo "Checking for linting errors..."
uv run ruff check

echo "Checking for format issues..."
uv run ruff format --check
