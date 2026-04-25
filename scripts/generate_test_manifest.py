import os
import json
import hashlib
from pathlib import Path

def get_sha256(file_path):
    sha256_hash = hashlib.sha256()
    with open(file_path, "rb") as f:
        for byte_block in iter(lambda: f.read(4096), b""):
            sha256_hash.update(byte_block)
    return sha256_hash.hexdigest()

def generate_manifest(source_dir, base_url, output_file):
    source_path = Path(source_dir)
    manifest = {
        "version": "9.9.10",
        "total_size_bytes": 0,
        "file_counts": {"plugin": 0, "backend_src": 0},
        "files": {"plugin": [], "backend_src": []}
    }

    for category in ["plugin", "backend_src"]:
        cat_path = source_path / category
        if not cat_path.exists():
            continue
        
        for file_path in cat_path.rglob("*"):
            if file_path.is_file():
                rel_path = file_path.relative_to(cat_path)
                size = file_path.stat().st_size
                sha = get_sha256(file_path)
                
                manifest["files"][category].append({
                    "path": str(rel_path),
                    "url": f"{base_url}/{category}/{rel_path}",
                    "sha256": sha,
                    "size_bytes": size
                })
                manifest["total_size_bytes"] += size
                manifest["file_counts"][category] += 1

    with open(output_file, "w") as f:
        json.dump(manifest, f, indent=4)
    print(f"Manifest generated: {output_file}")

if __name__ == "__main__":
    generate_manifest(
        "test_update_env/source_files",
        "http://localhost:8080",
        "test_update_env/source_files/update-manifest-test.json"
    )
