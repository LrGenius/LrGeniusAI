import base64
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
import tkinter as tk
from pathlib import Path
from threading import Thread
from tkinter import messagebox, ttk

import requests


def _log(msg: str) -> None:
    print(msg, flush=True)


def verify_sha256(content: bytes, expected_hash: str) -> bool:
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

        self.root = tk.Tk()
        self.root.title("LrGeniusAI Updater")
        self.root.resizable(False, False)

        self.label = tk.Label(self.root, text="Preparing update...", pady=10)
        self.label.pack()

        self.progress = ttk.Progressbar(
            self.root, orient="horizontal", length=300, mode="determinate"
        )
        self.progress.pack(pady=10)

        self.status_label = tk.Label(self.root, text="", font=("Arial", 10))
        self.status_label.pack()

        # Centre window after widgets are packed and geometry is known
        self.root.update_idletasks()
        width = self.root.winfo_reqwidth()
        height = self.root.winfo_reqheight()
        x = (self.root.winfo_screenwidth() // 2) - (width // 2)
        y = (self.root.winfo_screenheight() // 2) - (height // 2)
        self.root.geometry(f"{width}x{height}+{x}+{y}")

        # Bring window to the front on macOS
        self.root.lift()
        self.root.attributes("-topmost", True)
        self.root.after(500, lambda: self.root.attributes("-topmost", False))
        self.root.focus_force()

    def update_status(self, current, total, message):
        """Thread-safe: schedules the UI update on the main thread."""
        _log(f"[{current}/{total}] {message}")
        self.root.after(0, self._apply_status, current, total, message)

    def _apply_status(self, current, total, message):
        self.progress["maximum"] = max(total, 1)
        self.progress["value"] = current
        self.status_label.config(text=message)

    def _on_update_complete(self):
        """Runs on the main thread after all files have been applied."""
        _log("Update applied — restarting backend...")
        backend_root = Path(self.backend_root)
        entry_point = backend_root / "src" / "geniusai_server.py"
        if not entry_point.exists():
            _log(f"Backend entry point not found: {entry_point}")
            messagebox.showwarning(
                "Backend Not Found",
                f"The update was applied successfully, but the backend entry "
                f"point was not found at:\n{entry_point}\n\n"
                "Please restart Lightroom to activate the new version.",
            )
            self.root.destroy()
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
            messagebox.showinfo(
                "Success",
                "LrGeniusAI has been updated successfully.\nYou can now restart Lightroom.",
            )
        except Exception as e:
            _log(f"Backend restart failed: {e}")
            messagebox.showwarning(
                "Backend Restart Failed",
                f"The update was applied successfully, but the backend "
                f"could not be restarted automatically:\n\n{e}\n\n"
                "Please restart Lightroom to activate the new version.",
            )
        self.root.destroy()

    def _on_update_error(self, error_msg):
        """Runs on the main thread when the update worker raises."""
        _log(f"Update error: {error_msg}")
        messagebox.showerror(
            "Update Error", f"An error occurred during update:\n\n{error_msg}"
        )
        self.root.destroy()

    def run(self):
        _log("Starting updater...")
        Thread(target=self.perform_update, daemon=True).start()
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

            # Build tagged list so is_plugin lookup is O(1) per file, avoiding
            # dict equality membership bugs when plugin/backend entries collide.
            all_files = [(entry, True) for entry in plugin_files] + [
                (entry, False) for entry in backend_files
            ]
            total_files = len(all_files)
            _log(
                f"Files to update: {total_files} ({len(plugin_files)} plugin, {len(backend_files)} backend)"
            )

            temp_dir = Path(os.path.expanduser("~/.lrgeniusai/update_tmp"))
            # Clear any stale files from a previous aborted run
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

                if not verify_sha256(content, sha):
                    raise Exception(f"SHA256 mismatch for {rel_path}")

                safe_name = hashlib.md5(rel_path.encode()).hexdigest()
                temp_path = temp_dir / safe_name
                with open(temp_path, "wb") as f:
                    f.write(content)

                target_base = plugin_root if is_plugin else backend_root / "src"
                downloaded.append((temp_path, target_base / rel_path, sha))

            # 2. Apply phase — backup then overwrite
            self.update_status(total_files, total_files, "Applying changes...")
            time.sleep(1)

            applied_backups: list[Path] = []
            for temp_path, target_path, sha in downloaded:
                target_path.parent.mkdir(parents=True, exist_ok=True)
                bak_path = target_path.with_suffix(target_path.suffix + ".bak")
                if target_path.exists():
                    shutil.copy2(target_path, bak_path)
                    applied_backups.append(bak_path)
                shutil.copy2(temp_path, target_path)

                # Verify the copy landed intact
                if sha and not verify_sha256(target_path.read_bytes(), sha):
                    raise Exception(f"Post-copy SHA256 mismatch for {target_path.name}")

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

            # Hand off to the main thread for all remaining GUI work
            self.root.after(0, self._on_update_complete)

        except Exception as e:
            self.root.after(0, self._on_update_error, str(e))


if __name__ == "__main__":
    if len(sys.argv) < 4:
        print(
            "Usage: updater.py <manifest_json_path> <plugin_path> <backend_root>",
            flush=True,
        )
        sys.exit(1)

    # Usage: python updater.py <manifest_json_path> <plugin_path> <backend_root>
    gui = UpdaterGUI(sys.argv[1], sys.argv[2], sys.argv[3])
    gui.run()
