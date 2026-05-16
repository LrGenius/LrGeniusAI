"""Catalog chat HTTP routes."""

from __future__ import annotations

import time

from flask import Blueprint, jsonify, request

from config import logger

chat_bp = Blueprint("chat", __name__)


@chat_bp.route("/chat/session", methods=["POST"])
def create_session():
    data = request.get_json(silent=True) or {}
    catalog_id = data.get("catalog_id")
    provider = data.get("provider")
    model = data.get("model")

    if not provider:
        return jsonify(
            {"error": "provider is required", "results": None, "warning": None}
        ), 400

    try:
        from services import agent as agent_service

        result = agent_service.start_session(catalog_id, provider, model)
        return jsonify({"results": result, "error": None, "warning": None})
    except Exception as e:
        logger.error("create_session failed: %s", e, exc_info=True)
        return jsonify({"error": str(e), "results": None, "warning": None}), 500


@chat_bp.route("/chat/turn", methods=["POST"])
def post_turn():
    data = request.get_json(silent=True) or {}
    session_id = data.get("session_id")
    message = data.get("message", "").strip()

    if not session_id:
        return jsonify(
            {"error": "session_id is required", "results": None, "warning": None}
        ), 400
    if not message:
        return jsonify(
            {"error": "message is required", "results": None, "warning": None}
        ), 400

    try:
        from services import agent as agent_service

        turn_id = agent_service.run_turn(session_id, message)
        return jsonify(
            {"results": {"turn_id": turn_id}, "error": None, "warning": None}
        )
    except ValueError as e:
        return jsonify({"error": str(e), "results": None, "warning": None}), 404
    except RuntimeError as e:
        if "turn_in_progress" in str(e):
            return jsonify(
                {
                    "error": "A turn is already in progress for this session",
                    "results": None,
                    "warning": None,
                }
            ), 409
        logger.error("post_turn RuntimeError: %s", e, exc_info=True)
        return jsonify({"error": str(e), "results": None, "warning": None}), 500
    except Exception as e:
        logger.error("post_turn failed: %s", e, exc_info=True)
        return jsonify({"error": str(e), "results": None, "warning": None}), 500


@chat_bp.route("/chat/turn/<turn_id>/events", methods=["GET"])
def get_turn_events(turn_id: str):
    try:
        cursor = int(request.args.get("cursor", 0))
    except (ValueError, TypeError):
        cursor = 0

    from services import agent as agent_service

    # Long-poll: wait up to 10s for new events
    deadline = time.monotonic() + 10.0
    while True:
        result = agent_service.get_turn_events(turn_id, cursor)
        if result["events"] or result["done"]:
            return jsonify({"results": result, "error": None, "warning": None})
        if time.monotonic() >= deadline:
            return jsonify({"results": result, "error": None, "warning": None})
        time.sleep(0.25)


@chat_bp.route("/chat/commit", methods=["POST"])
def commit_proposal():
    data = request.get_json(silent=True) or {}
    session_id = data.get("session_id")
    proposal_id = data.get("proposal_id")

    if not session_id or not proposal_id:
        return jsonify(
            {
                "error": "session_id and proposal_id are required",
                "results": None,
                "warning": None,
            }
        ), 400

    try:
        from services import agent as agent_service

        result = agent_service.commit_proposal(session_id, proposal_id)
        return jsonify({"results": result, "error": None, "warning": None})
    except ValueError as e:
        return jsonify({"error": str(e), "results": None, "warning": None}), 400
    except Exception as e:
        logger.error("commit_proposal failed: %s", e, exc_info=True)
        return jsonify({"error": str(e), "results": None, "warning": None}), 500


@chat_bp.route("/chat/session/<session_id>", methods=["GET"])
def get_session_transcript(session_id: str):
    try:
        from services import agent as agent_service

        messages = agent_service.get_session_transcript(session_id)
        return jsonify(
            {"results": {"messages": messages}, "error": None, "warning": None}
        )
    except Exception as e:
        logger.error("get_session_transcript failed: %s", e, exc_info=True)
        return jsonify({"error": str(e), "results": None, "warning": None}), 500


@chat_bp.route("/chat/sessions", methods=["GET"])
def list_sessions():
    try:
        from services import chat_db

        sessions = chat_db.list_sessions()
        return jsonify(
            {"results": {"sessions": sessions}, "error": None, "warning": None}
        )
    except Exception as e:
        logger.error("list_sessions failed: %s", e, exc_info=True)
        return jsonify({"error": str(e), "results": None, "warning": None}), 500
