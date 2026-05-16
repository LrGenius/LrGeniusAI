"""SQLite persistence for catalog chat sessions and messages."""

from __future__ import annotations

import os
import sqlite3
import threading
from contextlib import contextmanager

import config
from config import logger

_lock = threading.Lock()
_conn: sqlite3.Connection | None = None

_SCHEMA = """
CREATE TABLE IF NOT EXISTS chat_sessions (
    session_id   TEXT PRIMARY KEY,
    catalog_id   TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    provider     TEXT,
    model        TEXT
);

CREATE TABLE IF NOT EXISTS chat_messages (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   TEXT NOT NULL REFERENCES chat_sessions(session_id),
    turn_id      TEXT NOT NULL,
    role         TEXT NOT NULL,
    content_json TEXT NOT NULL,
    created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_chat_messages_session ON chat_messages(session_id, id);
"""


def _db_path() -> str | None:
    if not config.DB_PATH:
        return None
    return os.path.join(config.DB_PATH, "chat.db")


def _get_conn() -> sqlite3.Connection | None:
    global _conn
    path = _db_path()
    if not path:
        return None
    if _conn is not None:
        return _conn
    with _lock:
        if _conn is not None:
            return _conn
        try:
            conn = sqlite3.connect(path, check_same_thread=False)
            conn.row_factory = sqlite3.Row
            conn.execute("PRAGMA journal_mode=WAL")
            conn.executescript(_SCHEMA)
            conn.commit()
            _conn = conn
            logger.info("Chat SQLite database initialized at %s", path)
        except Exception as e:
            logger.error("Failed to initialize chat database: %s", e, exc_info=True)
            return None
    return _conn


def reset() -> None:
    """Reset the connection so the next call re-opens against the current DB_PATH."""
    global _conn
    with _lock:
        if _conn is not None:
            try:
                _conn.close()
            except Exception:
                pass
            _conn = None


@contextmanager
def _cursor():
    conn = _get_conn()
    if conn is None:
        raise RuntimeError("Chat database not available (DB_PATH not set)")
    cur = conn.cursor()
    try:
        yield cur
        conn.commit()
    except Exception:
        conn.rollback()
        raise
    finally:
        cur.close()


# --- Session operations ---


def create_session(
    session_id: str,
    catalog_id: str | None,
    provider: str | None,
    model: str | None,
    now: int,
) -> None:
    with _cursor() as cur:
        cur.execute(
            "INSERT OR REPLACE INTO chat_sessions (session_id, catalog_id, created_at, updated_at, provider, model) VALUES (?,?,?,?,?,?)",
            (session_id, catalog_id, now, now, provider, model),
        )


def touch_session(session_id: str, now: int) -> None:
    with _cursor() as cur:
        cur.execute(
            "UPDATE chat_sessions SET updated_at=? WHERE session_id=?",
            (now, session_id),
        )


def get_session(session_id: str) -> dict | None:
    conn = _get_conn()
    if conn is None:
        return None
    cur = conn.cursor()
    cur.execute("SELECT * FROM chat_sessions WHERE session_id=?", (session_id,))
    row = cur.fetchone()
    cur.close()
    return dict(row) if row else None


def list_sessions(limit: int = 50) -> list[dict]:
    conn = _get_conn()
    if conn is None:
        return []
    cur = conn.cursor()
    cur.execute(
        "SELECT * FROM chat_sessions ORDER BY updated_at DESC LIMIT ?", (limit,)
    )
    rows = cur.fetchall()
    cur.close()
    return [dict(r) for r in rows]


# --- Message operations ---


def append_message(
    session_id: str, turn_id: str, role: str, content: dict, now: int
) -> int:
    import json

    with _cursor() as cur:
        cur.execute(
            "INSERT INTO chat_messages (session_id, turn_id, role, content_json, created_at) VALUES (?,?,?,?,?)",
            (session_id, turn_id, role, json.dumps(content), now),
        )
        return cur.lastrowid  # type: ignore[return-value]


def get_messages(session_id: str) -> list[dict]:
    import json

    conn = _get_conn()
    if conn is None:
        return []
    cur = conn.cursor()
    cur.execute(
        "SELECT * FROM chat_messages WHERE session_id=? ORDER BY id", (session_id,)
    )
    rows = cur.fetchall()
    cur.close()
    out = []
    for r in rows:
        d = dict(r)
        try:
            d["content"] = json.loads(d.pop("content_json"))
        except Exception:
            d["content"] = {}
        out.append(d)
    return out


def get_messages_for_turn(turn_id: str) -> list[dict]:
    import json

    conn = _get_conn()
    if conn is None:
        return []
    cur = conn.cursor()
    cur.execute("SELECT * FROM chat_messages WHERE turn_id=? ORDER BY id", (turn_id,))
    rows = cur.fetchall()
    cur.close()
    out = []
    for r in rows:
        d = dict(r)
        try:
            d["content"] = json.loads(d.pop("content_json"))
        except Exception:
            d["content"] = {}
        out.append(d)
    return out
