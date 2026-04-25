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

    def update_status(self, current, total, message):
        self.progress["maximum"] = max(total, 1)
        self.progress["value"] = current
        self.status_label.config(text=message)
        self.root.update()

    def run(self):
        Thread(target=self.perform_update, daemon=True).start()
        self.root.mainloop()

    def perform_update(self):
        try:
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

            # 5. Restart backend — inherit the full environment so that env-var
            #    configuration (GENIUSAI_PORT, etc.) is preserved across the restart.
            entry_point = backend_root / "src" / "geniusai_server.py"
            backend_restarted = False
            if entry_point.exists():
                try:
                    subprocess.Popen(
                        [sys.executable, str(entry_point)],
                        start_new_session=True,
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                        env=os.environ.copy(),
                        cwd=str(backend_root / "src"),
                    )
                    backend_restarted = True
                except Exception as e:
                    messagebox.showwarning(
                        "Backend Restart Failed",
                        f"The update was applied successfully, but the backend "
                        f"could not be restarted automatically:\n\n{e}\n\n"
                        "Please restart Lightroom to activate the new version.",
                    )
            else:
                messagebox.showwarning(
                    "Backend Not Found",
                    f"The update was applied successfully, but the backend entry "
                    f"point was not found at:\n{entry_point}\n\n"
                    "Please restart Lightroom to activate the new version.",
                )

            if backend_restarted:
                messagebox.showinfo(
                    "Success",
                    "LrGeniusAI has been updated successfully.\nYou can now restart Lightroom.",
                )
            self.root.destroy()

        except Exception as e:
            messagebox.showerror(
                "Update Error", f"An error occurred during update:\n\n{str(e)}"
            )
            self.root.destroy()


if __name__ == "__main__":
    if len(sys.argv) < 4:
        sys.exit(1)

    # Usage: python updater.py <manifest_json_path> <plugin_path> <backend_root>
    gui = UpdaterGUI(sys.argv[1], sys.argv[2], sys.argv[3])
    gui.run()
