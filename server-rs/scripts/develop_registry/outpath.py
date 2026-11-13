"""Output-path guard shared by the tools in this directory.

Everything these tools write from your own catalog, sidecars or training table
is private (paths, file names, photo ids, capture data). The repository is
public, so the tools refuse to write such output anywhere inside the
repository's working tree. The one exception is ``extract_fixtures.py``, whose
scrubbed output may go to ``server-rs/testdata/develop/``.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path

# Relative to the repository root; the only place scrubbed fixtures may land.
FIXTURE_ROOT = Path("server-rs") / "testdata" / "develop"


def repo_root() -> Path | None:
    """Root of the working tree this file lives in (``.git`` may be a file in a worktree)."""
    for parent in Path(__file__).resolve().parents:
        if (parent / ".git").exists():
            return parent
    return None


def _inside(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
    except ValueError:
        return False
    return True


def output_problem(path: str | os.PathLike, *, allow_fixtures: bool = False) -> str | None:
    """Why ``path`` must not receive output, or ``None`` when it may."""
    root = repo_root()
    if root is None:
        return None
    target = Path(path).expanduser().resolve()
    if not _inside(target, root):
        return None
    if allow_fixtures and _inside(target, root / FIXTURE_ROOT):
        return None
    where = f"{FIXTURE_ROOT}/" if allow_fixtures else "a folder outside the repository"
    return f"refusing to write into the repository ({target}); use {where}"


def require_outside_repo(path: str | os.PathLike, *, allow_fixtures: bool = False) -> None:
    """Exit with status 2 when ``path`` is inside the repository (see ``output_problem``)."""
    problem = output_problem(path, allow_fixtures=allow_fixtures)
    if problem:
        print(problem, file=sys.stderr)
        sys.exit(2)
