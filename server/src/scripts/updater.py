import base64
import datetime
import hashlib
import json
import os
import queue
import re
import shlex
import shutil
import subprocess
import sys
import time
import tkinter as tk
from pathlib import Path
from threading import Thread
import requests

# ── Modern dark palette ───────────────────────────────────────────────────────
_BG = "#1c1c1e"
_SURFACE = "#2c2c2e"
_ACCENT = "#0a84ff"
_TEXT = "#ffffff"
_SUBTEXT = "#8e8e93"
_SUCCESS = "#30d158"
_ERROR = "#ff453a"
_TRACK = "#3a3a3c"
_FONT_TITLE = ("Helvetica", 14, "bold")
_FONT_BODY = ("Helvetica", 11)
_FONT_SMALL = ("Helvetica", 10)
_BTN_HOVER = "#1a6fd4"
_W = 420
_PAD = 28


def _modern_dialog(title: str, message: str, kind: str = "info") -> None:
    """Modern dark-themed replacement for messagebox.show*."""
    win = tk.Tk()
    win.withdraw()
    win.title(title)
    win.resizable(False, False)
    win.configure(bg=_BG)

    outer = tk.Frame(win, bg=_BG, padx=_PAD, pady=_PAD)
    outer.pack(fill="both", expand=True)

    accent = {
        "info": (_ACCENT, "✓"),
        "warning": ("#ff9f0a", "!"),
        "error": (_ERROR, "✕"),
    }.get(kind, (_ACCENT, "i"))
    color, icon = accent

    circle = tk.Label(
        outer,
        text=icon,
        bg=color,
        fg=_TEXT,
        font=("Helvetica", 13, "bold"),
        width=2,
        height=1,
    )
    circle.pack(anchor="w", pady=(0, 14))

    tk.Label(outer, text=title, bg=_BG, fg=_TEXT, font=_FONT_TITLE, anchor="w").pack(
        fill="x", anchor="w"
    )
    tk.Label(
        outer,
        text=message,
        bg=_BG,
        fg=_SUBTEXT,
        font=_FONT_SMALL,
        wraplength=_W - _PAD * 2,
        justify="left",
        anchor="w",
    ).pack(fill="x", anchor="w", pady=(6, 20))

    btn = tk.Button(
        outer,
        text="OK",
        bg=_ACCENT,
        fg=_TEXT,
        font=("Helvetica", 11, "bold"),
        relief="flat",
        bd=0,
        padx=20,
        pady=8,
        cursor="hand2",
        command=win.destroy,
        activebackground=_BTN_HOVER,
        activeforeground=_TEXT,
    )
    btn.pack(anchor="e")

    win.update_idletasks()
    w, h = win.winfo_reqwidth(), win.winfo_reqheight()
    x = (win.winfo_screenwidth() // 2) - (w // 2)
    y = (win.winfo_screenheight() // 2) - (h // 2)
    win.geometry(f"{w}x{h}+{x}+{y}")
    win.deiconify()
    win.lift()
    win.attributes("-topmost", True)
    win.focus_force()
    win.mainloop()


def _patch_info_lua(content: bytes, version: str) -> bytes:
    """Bake the release version into downloaded Info.lua (repo has dev placeholders)."""
    parts = version.split(".")
    major = parts[0] if len(parts) > 0 else "0"
    minor = parts[1] if len(parts) > 1 else "0"
    revision = parts[2] if len(parts) > 2 else "0"
    build = datetime.date.today().strftime("%Y%m%d")
    text = content.decode("utf-8")
    text = re.sub(r"Info\.MAJOR\s*=\s*\d+", f"Info.MAJOR = {major}", text)
    text = re.sub(r"Info\.MINOR\s*=\s*\d+", f"Info.MINOR = {minor}", text)
    text = re.sub(r"Info\.REVISION\s*=\s*\d+", f"Info.REVISION = {revision}", text)
    text = re.sub(r"Info\.BUILD\s*=\s*\d+", f"Info.BUILD = {build}", text)
    return text.encode("utf-8")


def _log(msg: str) -> None:
    print(msg, flush=True)


def _apply_elevated(ops: list[tuple[Path, Path]]) -> None:
    """Run all (src, dst) copies in a single privileged osascript call (one prompt)."""
    # Write a temp shell script so escaping is trivial and the prompt fires once.
    script_path = Path(os.path.expanduser("~/.lrgeniusai/update_tmp/apply_elevated.sh"))
    script_path.parent.mkdir(parents=True, exist_ok=True)
    with open(script_path, "w") as f:
        f.write("#!/bin/bash\nset -e\n")
        for src, dst in ops:
            f.write(f"cp -p {shlex.quote(str(src))} {shlex.quote(str(dst))}\n")
    script_path.chmod(0o700)

    # AppleScript requires double-quoted strings — shlex.quote uses single quotes and breaks.
    escaped = str(script_path).replace("\\", "\\\\").replace('"', '\\"')
    osascript_cmd = f'do shell script "{escaped}" with administrator privileges'
    result = subprocess.run(
        ["osascript", "-e", osascript_cmd], capture_output=True, text=True
    )
    if result.returncode != 0:
        raise PermissionError(f"Privileged apply failed: {result.stderr.strip()}")


def verify_sha256(content: bytes, expected_hash: str | None) -> bool:
    if not expected_hash:
        return True
    actual_hash = hashlib.sha256(content).hexdigest()
    return actual_hash.lower() == expected_hash.lower()


def download_with_retry(url: str, timeout: int = 30, retries: int = 3) -> bytes:
    delay = 2
    for attempt in range(retries):
        try:
            resp = requests.get(url, timeout=timeout)
            if resp.status_code == 200:
                return resp.content
            raise Exception(f"HTTP {resp.status_code}")
        except Exception:
            if attempt == retries - 1:
                raise
            time.sleep(delay)
            delay *= 2
    raise Exception("Download failed after retries")


class UpdaterGUI:
    def __init__(self, manifest_path, plugin_path, backend_root):
        self.manifest_path = manifest_path
        self.plugin_path = plugin_path
        self.backend_root = backend_root
        # Queue used to pass events from the worker thread to the main thread.
        # Tuples: ('status', current, total, msg) | ('done',) | ('error', msg)
        self._q: queue.Queue = queue.Queue()
        self._bar_total = 1
        self._bar_value = 0

        self.root = tk.Tk()
        self.root.title("LrGeniusAI Updater")
        self.root.resizable(False, False)
        self.root.configure(bg=_BG)

        outer = tk.Frame(self.root, bg=_BG, padx=_PAD, pady=_PAD)
        outer.pack(fill="both", expand=True)

        # App name + subtitle row
        tk.Label(
            outer,
            text="LrGeniusAI",
            bg=_BG,
            fg=_TEXT,
            font=_FONT_TITLE,
            anchor="w",
        ).pack(fill="x")

        self.label = tk.Label(
            outer,
            text="Preparing update…",
            bg=_BG,
            fg=_SUBTEXT,
            font=_FONT_BODY,
            anchor="w",
        )
        self.label.pack(fill="x", pady=(2, 20))

        # Canvas-based thin progress bar
        bar_bg = tk.Frame(outer, bg=_SURFACE, height=6)
        bar_bg.pack(fill="x")
        bar_bg.pack_propagate(False)
        self._bar_canvas = tk.Canvas(
            bar_bg, bg=_SURFACE, height=6, highlightthickness=0, bd=0
        )
        self._bar_canvas.pack(fill="both", expand=True)
        self._bar_fill = self._bar_canvas.create_rectangle(
            0, 0, 0, 6, fill=_ACCENT, outline=""
        )

        # File name label
        self.status_label = tk.Label(
            outer,
            text="",
            bg=_BG,
            fg=_SUBTEXT,
            font=_FONT_SMALL,
            anchor="w",
        )
        self.status_label.pack(fill="x", pady=(10, 0))

        # Kick off bar animation loop
        self.root.after(50, self._animate_bar)

        # Centre window
        self.root.update_idletasks()
        w = max(self.root.winfo_reqwidth(), _W)
        h = self.root.winfo_reqheight()
        x = (self.root.winfo_screenwidth() // 2) - (w // 2)
        y = (self.root.winfo_screenheight() // 2) - (h // 2)
        self.root.geometry(f"{w}x{h}+{x}+{y}")

        # Bring window to the front on macOS
        self.root.lift()
        self.root.attributes("-topmost", True)
        self.root.after(500, lambda: self.root.attributes("-topmost", False))
        self.root.focus_force()

    def _animate_bar(self):
        """Smoothly resize the progress fill on the canvas."""
        try:
            canvas_w = self._bar_canvas.winfo_width()
            filled = int(canvas_w * self._bar_value / max(self._bar_total, 1))
            self._bar_canvas.coords(self._bar_fill, 0, 0, filled, 6)
            self.root.after(50, self._animate_bar)
        except tk.TclError:
            pass  # window already destroyed

    def update_status(self, current, total, message):
        """Thread-safe: enqueues a status update for the main thread to pick up."""
        _log(f"[{current}/{total}] {message}")
        self._q.put(("status", current, total, message))

    def _poll_queue(self):
        """Called on the main thread every 100 ms to drain the worker queue."""
        try:
            while True:
                item = self._q.get_nowait()
                kind = item[0]
                if kind == "status":
                    _, current, total, message = item
                    self._bar_total = max(total, 1)
                    self._bar_value = current
                    self.status_label.config(text=message)
                elif kind == "done":
                    self._on_update_complete()
                    return  # stop polling — window will be destroyed
                elif kind == "error":
                    self._on_update_error(item[1])
                    return  # stop polling — window will be destroyed
        except queue.Empty:
            pass
        # Reschedule on the main thread
        self.root.after(100, self._poll_queue)

    def _on_update_complete(self):
        """Runs on the main thread after all files have been applied."""
        _log("Update applied — restarting backend...")
        backend_root = Path(self.backend_root)
        entry_point = backend_root / "src" / "geniusai_server.py"
        if not entry_point.exists():
            _log(f"Backend entry point not found: {entry_point}")
            self.root.destroy()
            _modern_dialog(
                "Backend Not Found",
                f"Update applied, but the backend entry point was not found at:\n{entry_point}\n\nRestart Lightroom to activate the new version.",
                "warning",
            )
            return

        try:
            subprocess.Popen(
                [sys.executable, str(entry_point)],
                start_new_session=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                env=os.environ.copy(),
                cwd=str(backend_root / "src"),
            )
            _log("Backend restarted successfully.")
            self.root.destroy()
            _modern_dialog(
                "Update complete",
                "LrGeniusAI has been updated successfully. You can now restart Lightroom.",
                "info",
            )
        except Exception as e:
            _log(f"Backend restart failed: {e}")
            self.root.destroy()
            _modern_dialog(
                "Backend restart failed",
                f"The update was applied, but the backend could not restart automatically:\n\n{e}\n\nRestart Lightroom to activate the new version.",
                "warning",
            )

    def _on_update_error(self, error_msg):
        """Runs on the main thread when the update worker raises."""
        _log(f"Update error: {error_msg}")
        self.root.destroy()
        _modern_dialog(
            "Update failed",
            f"An error occurred during the update:\n\n{error_msg}",
            "error",
        )

    def run(self):
        _log("Starting updater...")
        Thread(target=self.perform_update, daemon=True).start()
        self.root.after(100, self._poll_queue)
        self.root.mainloop()
        _log("Updater window closed.")

    def perform_update(self):
        try:
            _log(f"Reading manifest: {self.manifest_path}")
            with open(self.manifest_path, "r") as f:
                manifest = json.load(f)

            plugin_root = Path(self.plugin_path)
            backend_root = Path(self.backend_root)

            files = manifest.get("files", {})
            plugin_files = files.get("plugin", [])
            backend_files = files.get("backend_src", [])

            all_files = [(entry, True) for entry in plugin_files] + [
                (entry, False) for entry in backend_files
            ]
            total_files = len(all_files)
            _log(
                f"Files to update: {total_files} ({len(plugin_files)} plugin, {len(backend_files)} backend)"
            )

            temp_dir = Path(os.path.expanduser("~/.lrgeniusai/update_tmp"))
            if temp_dir.exists():
                shutil.rmtree(temp_dir)
            temp_dir.mkdir(parents=True, exist_ok=True)

            downloaded: list[tuple[Path, Path, str | None]] = []

            # 1. Download (or decode inline content) phase
            for i, (entry, is_plugin) in enumerate(all_files):
                rel_path = entry["path"]
                sha = entry.get("sha256")

                self.update_status(
                    i, total_files, f"Downloading {os.path.basename(rel_path)}..."
                )

                if "content" in entry:
                    content = base64.b64decode(entry["content"])
                else:
                    content = download_with_retry(entry["url"])

                # Info.lua is downloaded from the repo tag where version fields are
                # still dev placeholders — patch them to the real release version.
                if is_plugin and rel_path == "Info.lua":
                    version = manifest.get("version", "")
                    if version:
                        content = _patch_info_lua(content, version)
                        sha = None  # sha was for the placeholder; skip check after patching

                if not verify_sha256(content, sha):
                    raise Exception(f"SHA256 mismatch for {rel_path}")

                safe_name = hashlib.md5(rel_path.encode()).hexdigest()
                temp_path = temp_dir / safe_name
                with open(temp_path, "wb") as f:
                    f.write(content)

                target_base = plugin_root if is_plugin else backend_root / "src"
                downloaded.append((temp_path, target_base / rel_path, sha))

            # 2. Apply phase — split by write permission
            self.update_status(total_files, total_files, "Applying changes...")
            time.sleep(1)

            normal: list[tuple[Path, Path, str | None]] = []
            elevated: list[tuple[Path, Path, str | None]] = []
            for temp_path, target_path, sha in downloaded:
                target_path.parent.mkdir(parents=True, exist_ok=True)
                if os.access(target_path.parent, os.W_OK):
                    normal.append((temp_path, target_path, sha))
                else:
                    elevated.append((temp_path, target_path, sha))

            # Normal files — direct copy with in-place backup
            applied_backups: list[Path] = []
            for temp_path, target_path, sha in normal:
                _log(f"  applying {target_path}")
                bak_path = target_path.with_suffix(target_path.suffix + ".bak")
                if target_path.exists():
                    shutil.copy2(target_path, bak_path)
                    applied_backups.append(bak_path)
                shutil.copy2(temp_path, target_path)
                if sha and not verify_sha256(target_path.read_bytes(), sha):
                    raise Exception(f"Post-copy SHA256 mismatch for {target_path.name}")

            # Elevated files — single admin prompt for all of them
            if elevated:
                _log(
                    f"  {len(elevated)} files need admin privileges — requesting once..."
                )
                ops = []
                for temp_path, target_path, sha in elevated:
                    _log(f"  applying {target_path}")
                    ops.append((temp_path, target_path))
                _apply_elevated(ops)
                for _, target_path, sha in elevated:
                    if sha and not verify_sha256(target_path.read_bytes(), sha):
                        raise Exception(
                            f"Post-copy SHA256 mismatch for {target_path.name}"
                        )

            # 3. Remove backup files on full success
            for bak_path in applied_backups:
                try:
                    bak_path.unlink()
                except Exception:
                    pass

            # 4. Cleanup temp dir
            try:
                shutil.rmtree(temp_dir)
            except Exception:
                pass

            self.update_status(total_files, total_files, "Update complete!")
            self._q.put(("done",))

        except Exception as e:
            _log(f"Update error: {e}")
            self._q.put(("error", str(e)))


if __name__ == "__main__":
    if len(sys.argv) < 4:
        print(
            "Usage: updater.py <manifest_json_path> <plugin_path> <backend_root>",
            flush=True,
        )
        sys.exit(1)

    gui = UpdaterGUI(sys.argv[1], sys.argv[2], sys.argv[3])
    gui.run()
