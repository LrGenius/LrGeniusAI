"""Catalog chat agent: session management and turn execution."""

from __future__ import annotations

import json
import threading
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any

from config import logger
from services import agent_tools as tools_module
from services import chat_db
from services import db as db_service

MAX_ITERS = 8

# Per-session thread pool (max 1 concurrent turn)
_session_executors: dict[str, ThreadPoolExecutor] = {}
_session_executors_lock = threading.Lock()

# In-flight turn futures: turn_id -> future
_turn_futures: dict[str, Any] = {}

# Event ring per turn: turn_id -> list of events
_turn_events: dict[str, list[dict]] = {}
_turn_done: dict[str, bool] = {}
_turn_events_lock = threading.Lock()


@dataclass
class Session:
    session_id: str
    catalog_id: str | None
    provider: str | None
    model: str | None
    created_at: int
    # Accumulated retrieved photo_ids this session (for proposal guard)
    retrieved_photo_ids: set[str] = field(default_factory=set)
    # result_refs: opaque key -> full list[str] of photo_ids
    result_refs: dict[str, list[str]] = field(default_factory=dict)
    # Whether a turn is currently running
    turn_in_progress: bool = False
    turn_lock: threading.Lock = field(default_factory=threading.Lock)


SESSIONS: dict[str, Session] = {}
_sessions_lock = threading.Lock()


def _now_ms() -> int:
    return int(time.time() * 1000)


def _now_s() -> int:
    return int(time.time())


def start_session(
    catalog_id: str | None, provider: str | None, model: str | None
) -> dict:
    session_id = str(uuid.uuid4())
    now = _now_s()
    sess = Session(
        session_id=session_id,
        catalog_id=catalog_id,
        provider=provider,
        model=model,
        created_at=now,
    )
    with _sessions_lock:
        SESSIONS[session_id] = sess

    chat_db.create_session(session_id, catalog_id, provider, model, now)

    # Build a brief catalog summary for the system prompt
    try:
        stats = db_service.get_database_stats(catalog_id=catalog_id)
        photo_count = stats.get("photos", {}).get("total", 0)
        system_summary = f"{photo_count} photos indexed"
    except Exception as e:
        logger.warning("Could not fetch stats for session %s: %s", session_id, e)
        system_summary = "catalog stats unavailable"

    logger.info(
        "Started chat session %s (provider=%s, model=%s, catalog=%s)",
        session_id,
        provider,
        model,
        catalog_id,
    )
    return {"session_id": session_id, "system_summary": system_summary}


def _get_session(session_id: str) -> Session | None:
    with _sessions_lock:
        sess = SESSIONS.get(session_id)
    if sess is None:
        # Try to restore from DB
        row = chat_db.get_session(session_id)
        if row:
            sess = Session(
                session_id=session_id,
                catalog_id=row.get("catalog_id"),
                provider=row.get("provider"),
                model=row.get("model"),
                created_at=row.get("created_at", _now_s()),
            )
            with _sessions_lock:
                SESSIONS[session_id] = sess
    return sess


def _push_event(turn_id: str, kind: str, payload: dict) -> None:
    with _turn_events_lock:
        events = _turn_events.setdefault(turn_id, [])
        seq = len(events)
        events.append({"seq": seq, "kind": kind, "payload": payload})


def _mark_turn_done(turn_id: str) -> None:
    with _turn_events_lock:
        _turn_done[turn_id] = True


def get_turn_events(turn_id: str, cursor: int) -> dict:
    with _turn_events_lock:
        events = _turn_events.get(turn_id, [])
        done = _turn_done.get(turn_id, False)
        slice_ = events[cursor:]
    return {
        "events": slice_,
        "next_cursor": cursor + len(slice_),
        "done": done,
    }


def _build_system_prompt(sess: Session) -> str:
    try:
        stats = db_service.get_database_stats(catalog_id=sess.catalog_id)
        photo_count = stats.get("photos", {}).get("total", 0)
        with_desc = stats.get("photos", {}).get("with_caption", 0)
        catalog_summary = (
            f"{photo_count} photos total, {with_desc} with AI descriptions"
        )
    except Exception:
        catalog_summary = "catalog stats unavailable"

    return (
        "You are an assistant inside Adobe Lightroom Classic helping the user query and act on their photo catalog.\n\n"
        f"Catalog overview: {catalog_summary}\n\n"
        "Hard rules:\n"
        "- You cannot modify the catalog directly. To make changes, call `propose_action` with a clear `dry_run_summary`. The user will confirm or reject.\n"
        "- Always cite the tool result that produced any photo_id you reference.\n"
        "- Prefer narrow `metadata_query` + `semantic_search` over broad lists. Use `scope_photo_ids` or `result_ref` to intersect.\n"
        "- Never invent photo_ids, person_ids, or keywords. If a lookup returns nothing, say so.\n"
        "- When photo lists are long, summarize rather than listing every ID."
    )


def _get_provider(sess: Session):
    """Return an initialized LLM provider for the session."""
    import config as cfg

    provider_name = (sess.provider or "").lower()
    api_key = None

    if provider_name in ("chatgpt", "openai"):
        api_key = cfg.OPENAI_API_KEY if hasattr(cfg, "OPENAI_API_KEY") else None
        from providers.chatgpt import ChatGPTProvider

        return ChatGPTProvider({"api_key": api_key})
    elif provider_name == "gemini":
        api_key = cfg.GEMINI_API_KEY if hasattr(cfg, "GEMINI_API_KEY") else None
        from providers.gemini import GeminiProvider

        return GeminiProvider({"api_key": api_key})
    elif provider_name == "lmstudio":
        base_url = (
            cfg.LMSTUDIO_BASE_URL
            if hasattr(cfg, "LMSTUDIO_BASE_URL")
            else "http://localhost:1234"
        )
        from providers.lmstudio import LMStudioProvider

        return LMStudioProvider({"lmstudio_base_url": base_url})
    elif provider_name == "ollama":
        base_url = (
            cfg.OLLAMA_BASE_URL
            if hasattr(cfg, "OLLAMA_BASE_URL")
            else "http://localhost:11434"
        )
        from providers.ollama import OllamaProvider

        return OllamaProvider({"ollama_base_url": base_url})
    else:
        raise ValueError(f"Unknown provider: {sess.provider!r}")


def _run_turn_background(session_id: str, turn_id: str, user_text: str) -> None:
    """Executed in a background thread. Drives the agent loop for one turn."""
    sess = _get_session(session_id)
    if sess is None:
        _push_event(turn_id, "error", {"message": "Session not found"})
        _mark_turn_done(turn_id)
        return

    now = _now_s()
    chat_db.append_message(session_id, turn_id, "user", {"text": user_text}, now)
    chat_db.touch_session(session_id, now)

    try:
        provider = _get_provider(sess)
    except Exception as e:
        logger.error(
            "Could not initialize provider for session %s: %s",
            session_id,
            e,
            exc_info=True,
        )
        _push_event(turn_id, "error", {"message": f"Provider init failed: {e}"})
        _mark_turn_done(turn_id)
        with sess.turn_lock:
            sess.turn_in_progress = False
        return

    # Build full message history from DB
    history_rows = chat_db.get_messages(session_id)
    from providers.base import ChatMessage

    system_prompt = _build_system_prompt(sess)

    messages: list[ChatMessage] = [ChatMessage(role="system", content=system_prompt)]
    for row in history_rows:
        role = row["role"]
        content_obj = row["content"]
        if role == "user":
            messages.append(
                ChatMessage(role="user", content=content_obj.get("text", ""))
            )
        elif role == "assistant":
            messages.append(
                ChatMessage(
                    role="assistant",
                    content=content_obj.get("text"),
                    tool_calls=content_obj.get("tool_calls"),
                )
            )
        elif role == "tool":
            messages.append(
                ChatMessage(
                    role="tool",
                    content=json.dumps(content_obj.get("result", {})),
                    tool_call_id=content_obj.get("tool_call_id"),
                )
            )
        # skip proposal rows in the LLM history

    tools_list = list(tools_module.TOOLS.values())

    iters = 0

    while iters < MAX_ITERS:
        iters += 1
        try:
            events_iter = provider.chat_with_tools(
                messages,
                tools_list,
                model=sess.model,
                temperature=0.2,
            )
        except Exception as e:
            logger.error(
                "Provider chat_with_tools failed (iter %d): %s", iters, e, exc_info=True
            )
            _push_event(turn_id, "error", {"message": f"LLM error: {e}"})
            _mark_turn_done(turn_id)
            with sess.turn_lock:
                sess.turn_in_progress = False
            return

        assistant_text = None
        tool_calls_this_turn = []

        for event in events_iter:
            if event.kind == "tool_call":
                tc = event.payload  # dict with id, name, arguments
                _push_event(
                    turn_id,
                    "tool_call",
                    {
                        "tool": tc["name"],
                        "args_preview": _args_preview(tc["arguments"]),
                        "tool_call_id": tc["id"],
                    },
                )
                tool_calls_this_turn.append(tc)
            elif event.kind == "assistant_text":
                assistant_text = event.payload.get("text", "")
            elif event.kind == "proposal":
                prop = event.payload
                _push_event(turn_id, "proposal", prop)
                # Persist proposal as a message
                chat_db.append_message(session_id, turn_id, "proposal", prop, _now_s())

        if not tool_calls_this_turn:
            # No more tool calls — we're done
            if assistant_text:
                _push_event(turn_id, "assistant_text", {"text": assistant_text})
                chat_db.append_message(
                    session_id, turn_id, "assistant", {"text": assistant_text}, _now_s()
                )
            break

        # Record assistant turn with tool calls
        tc_serializable = [
            {"id": tc["id"], "name": tc["name"], "arguments": tc["arguments"]}
            for tc in tool_calls_this_turn
        ]
        messages.append(
            ChatMessage(
                role="assistant",
                content=assistant_text,
                tool_calls=tc_serializable,
            )
        )
        chat_db.append_message(
            session_id,
            turn_id,
            "assistant",
            {
                "text": assistant_text,
                "tool_calls": tc_serializable,
            },
            _now_s(),
        )

        # Dispatch each tool call
        for tc in tool_calls_this_turn:
            t_start = time.monotonic()
            try:
                result = tools_module.dispatch(tc["name"], tc["arguments"], sess)
            except Exception as e:
                logger.error(
                    "Tool dispatch error tool=%s: %s", tc["name"], e, exc_info=True
                )
                result = {"error": str(e)}
            elapsed = time.monotonic() - t_start
            logger.info(
                "tool=%s args=%s elapsed=%.3fs", tc["name"], tc["arguments"], elapsed
            )

            result_ref = result.get("result_ref")
            summary_text = _make_summary_text(result, tc["name"])

            _push_event(
                turn_id,
                "tool_result",
                {
                    "tool_call_id": tc["id"],
                    "summary_text": summary_text,
                    "result_ref": result_ref,
                },
            )

            tool_msg_content = json.dumps(result)
            messages.append(
                ChatMessage(
                    role="tool",
                    content=tool_msg_content,
                    tool_call_id=tc["id"],
                )
            )
            chat_db.append_message(
                session_id,
                turn_id,
                "tool",
                {
                    "tool_call_id": tc["id"],
                    "result": result,
                },
                _now_s(),
            )

    else:
        # Budget exhausted
        messages.append(
            ChatMessage(
                role="system",
                content="Tool budget exhausted. Finalize your answer now based on what you have retrieved.",
            )
        )
        try:
            for event in provider.chat_with_tools(
                messages, [], model=sess.model, temperature=0.2
            ):
                if event.kind == "assistant_text":
                    text = event.payload.get("text", "")
                    _push_event(turn_id, "assistant_text", {"text": text})
                    chat_db.append_message(
                        session_id, turn_id, "assistant", {"text": text}, _now_s()
                    )
        except Exception as e:
            logger.error(
                "Final turn after budget exhaustion failed: %s", e, exc_info=True
            )

    _push_event(turn_id, "done", {})
    _mark_turn_done(turn_id)

    with sess.turn_lock:
        sess.turn_in_progress = False


def _args_preview(args: dict) -> str:
    try:
        s = json.dumps(args)
        return s[:120] + "…" if len(s) > 120 else s
    except Exception:
        return str(args)[:120]


def _make_summary_text(result: dict, tool_name: str) -> str:
    if result.get("error"):
        return f"Error: {result['error']}"
    summary = result.get("summary", {})
    if isinstance(summary, dict):
        count = summary.get("count")
        if count is not None:
            return f"{count} result(s)"
    photo_ids = result.get("photo_ids", [])
    if isinstance(photo_ids, list) and photo_ids:
        return f"{len(photo_ids)} photo(s)"
    return "ok"


def run_turn(session_id: str, user_text: str) -> str:
    """Start a background turn. Returns turn_id immediately."""
    sess = _get_session(session_id)
    if sess is None:
        raise ValueError(f"Session not found: {session_id}")

    with sess.turn_lock:
        if sess.turn_in_progress:
            raise RuntimeError("turn_in_progress")
        sess.turn_in_progress = True

    turn_id = str(uuid.uuid4())

    with _session_executors_lock:
        if session_id not in _session_executors:
            _session_executors[session_id] = ThreadPoolExecutor(max_workers=1)
        executor = _session_executors[session_id]

    future = executor.submit(_run_turn_background, session_id, turn_id, user_text)
    _turn_futures[turn_id] = future
    return turn_id


def commit_proposal(session_id: str, proposal_id: str) -> dict:
    """Validate and return a proposal so the plugin can apply it."""
    sess = _get_session(session_id)
    if sess is None:
        raise ValueError(f"Session not found: {session_id}")

    messages = chat_db.get_messages(session_id)
    proposal = None
    for msg in messages:
        if msg["role"] == "proposal":
            content = msg["content"]
            if content.get("proposal_id") == proposal_id:
                proposal = content
                break

    if proposal is None:
        raise ValueError(
            f"Proposal {proposal_id!r} not found in session {session_id!r}"
        )

    # Validate photo_ids against retrieved set (prompt injection guard)
    payload_ids = set(proposal.get("payload", {}).get("photo_ids", []))
    if payload_ids and not payload_ids.issubset(sess.retrieved_photo_ids):
        unknown = payload_ids - sess.retrieved_photo_ids
        raise ValueError(
            f"Proposal references photo_ids not retrieved this session: {list(unknown)[:5]}"
        )

    return {
        "ok": True,
        "action": {"kind": proposal["kind"], "payload": proposal.get("payload", {})},
    }


def get_session_transcript(session_id: str) -> list[dict]:
    return chat_db.get_messages(session_id)
