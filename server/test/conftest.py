import sys
import os
import tempfile

# Monkeypatch sys.argv BEFORE any test modules are collected.
# This prevents config.py from crashing on argparse missing required args
# and ensures the log directory exists across all platforms (e.g. Windows).
sys.argv = ["pytest", "--db-path", os.path.join(tempfile.gettempdir(), "mock_db_path")]

# Add src and the server root to path so all tests can find the backend modules
# (supports both 'from src.xxx' and direct 'from xxx' imports)
_root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
sys.path.append(_root)
sys.path.append(os.path.join(_root, "src"))
