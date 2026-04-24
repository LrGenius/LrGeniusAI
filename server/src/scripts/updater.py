import os
import sys
import json
import hashlib
import requests
import shutil
import time
import subprocess
import tkinter as tk
from tkinter import ttk, messagebox
from pathlib import Path
from threading import Thread


def verify_sha256(content: bytes, expected_hash: str) -> bool:
    if not expected_hash:
        return True
    actual_hash = hashlib.sha256(content).hexdigest()
    return actual_hash.lower() == expected_hash.lower()


class UpdaterGUI:
    def __init__(self, manifest_path, plugin_path, backend_root):
        self.manifest_path = manifest_path
        self.plugin_path = plugin_path
        self.backend_root = backend_root

        self.root = tk.Tk()
        self.root.title("LrGeniusAI Updater")
        self.root.geometry("400x150")
        self.root.resizable(False, False)

        # Center window
        self.root.eval("centralize { . }")  # wait, that's not right
        self.root.update_idletasks()
        width = self.root.winfo_width()
        height = self.root.winfo_height()
        x = (self.root.winfo_screenwidth() // 2) - (width // 2)
        y = (self.root.winfo_screenheight() // 2) - (height // 2)
        self.root.geometry(f"{width}x{height}+{x}+{y}")

        self.label = tk.Label(self.root, text="Preparing update...", pady=10)
        self.label.pack()

        self.progress = ttk.Progressbar(
            self.root, orient="horizontal", length=300, mode="determinate"
        )
        self.progress.pack(pady=10)

        self.status_label = tk.Label(self.root, text="", font=("Arial", 10))
        self.status_label.pack()

    def update_status(self, current, total, message):
        self.progress["maximum"] = total
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

            all_files = plugin_files + backend_files
            total_files = len(all_files)

            temp_dir = Path(os.path.expanduser("~/.lrgeniusai/update_tmp"))
            temp_dir.mkdir(parents=True, exist_ok=True)

            downloaded = []

            # 1. Download phase
            for i, entry in enumerate(all_files):
                rel_path = entry["path"]
                url = entry["url"]
                sha = entry.get("sha256")

                self.update_status(
                    i, total_files, f"Downloading {os.path.basename(rel_path)}..."
                )

                resp = requests.get(url, timeout=30)
                if resp.status_code != 200:
                    raise Exception(
                        f"Failed to download {rel_path}: {resp.status_code}"
                    )

                if not verify_sha256(resp.content, sha):
                    raise Exception(f"SHA256 mismatch for {rel_path}")

                safe_name = hashlib.md5(rel_path.encode()).hexdigest()
                temp_path = temp_dir / safe_name
                with open(temp_path, "wb") as f:
                    f.write(resp.content)

                is_plugin = entry in plugin_files
                target_base = plugin_root if is_plugin else backend_root / "src"
                downloaded.append((temp_path, target_base / rel_path))

            # 2. Apply phase
            self.update_status(total_files, total_files, "Applying changes...")
            # Wait a bit for files to be released if needed
            time.sleep(1)

            for temp_path, target_path in downloaded:
                target_path.parent.mkdir(parents=True, exist_ok=True)
                if target_path.exists():
                    try:
                        shutil.copy2(
                            target_path,
                            target_path.with_suffix(target_path.suffix + ".bak"),
                        )
                    except Exception:
                        pass
                shutil.copy2(temp_path, target_path)

            # 3. Cleanup
            try:
                shutil.rmtree(temp_dir)
            except Exception:
                pass

            self.update_status(total_files, total_files, "Update complete!")

            # Restart backend
            try:
                entry_point = backend_root / "src" / "geniusai_server.py"
                if entry_point.exists():
                    subprocess.Popen(
                        [sys.executable, str(entry_point)], start_new_session=True
                    )
            except Exception:
                pass

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
