"""M6 golden fixtures: run the real group_and_sort_images against a
synthetic Chroma collection with controlled capture_time/phash/embedding/
culling-metric values, to verify the Rust port end-to-end."""
import json, os, sys, shutil
sys.path.insert(0, "/Users/bm/src/LrGeniusAI/server/src")
sys.argv = ["x", "--db-path", "/tmp/lrg-grouping-golden-db"]

shutil.rmtree("/tmp/lrg-grouping-golden-db", ignore_errors=True)

import config
from services import chroma as chroma_service

chroma_service.ensure_db_path("/tmp/lrg-grouping-golden-db")

DIM = 1152

def phash_hex(n):
    return f"{n:016x}"

def embedding(seed_bits, dim=DIM):
    # Deterministic pseudo-embedding: mostly zeros with a few set bits at
    # seed-derived positions, then L2-normalized -- lets us construct
    # precise pairwise cosine distances via controlled overlap.
    import numpy as np
    rng = np.random.default_rng(seed_bits)
    v = rng.standard_normal(dim).astype("float32")
    v /= (v**2).sum() ** 0.5
    return v.tolist()

photos = [
    # A near-duplicate burst pair (close time, close phash, close embedding)
    dict(photo_id="p1", capture_time=1000.0, filename="a.jpg",
         cull_phash=phash_hex(0x0000000000000000),
         cull_sharpness=0.9, cull_exposure=0.6, cull_noise=0.1,
         cull_highlight_clip=0.0, cull_shadow_clip=0.0,
         cull_technical_score=0.85, cull_aesthetic=0.7,
         embedding_seed=1),
    dict(photo_id="p2", capture_time=1000.5, filename="b.jpg",
         cull_phash=phash_hex(0x0000000000000001),  # hamming 1 from p1
         cull_sharpness=0.4, cull_exposure=0.55, cull_noise=0.3,
         cull_highlight_clip=0.0, cull_shadow_clip=0.0,
         cull_technical_score=0.5, cull_aesthetic=0.6,
         embedding_seed=1),  # same seed -> ~identical embedding (dist~0)
    # An isolated single photo, far away in time
    dict(photo_id="p3", capture_time=5000.0, filename="c.jpg",
         cull_phash=phash_hex(0xFFFFFFFFFFFFFFFF),
         cull_sharpness=0.8, cull_exposure=0.5, cull_noise=0.05,
         cull_highlight_clip=0.0, cull_shadow_clip=0.0,
         cull_technical_score=0.8, cull_aesthetic=0.5,
         embedding_seed=99),
    # A photo with no capture_time and no phash (edge case)
    dict(photo_id="p4", capture_time=None, filename="d.jpg",
         cull_phash=None,
         cull_sharpness=0.2, cull_exposure=0.2, cull_noise=0.5,
         cull_highlight_clip=0.1, cull_shadow_clip=0.1,
         cull_technical_score=0.2, cull_aesthetic=0.2,
         embedding_seed=50),
]

for p in photos:
    meta = {k: v for k, v in p.items() if k not in ("embedding_seed",) and v is not None}
    meta["has_embedding"] = True
    chroma_service.add_image(p["photo_id"], embedding(p["embedding_seed"]), meta)

photo_ids = [p["photo_id"] for p in photos]
groups = chroma_service.group_and_sort_images(photo_ids, "auto", "auto", 1, culling_preset="default")

out = json.dumps(groups, default=str, indent=1)
dst = "/Users/bm/src/LrGeniusAI/server-rs/testdata/grouping_goldens.json"
with open(dst, "w") as f:
    f.write(out)
print("groups:", len(groups))
for g in groups:
    print(g["group_id"], g["group_type"], g["photo_ids"], "winner:", g["winner_photo_id"])
    for ph in g["photos"]:
        print("   ", ph["photo_id"], "rank", ph["rank"], "score", ph["cull_score"], "reject", ph["reject_candidate"], ph["reason_codes"])
print("wrote", dst)

# Also dump the exact input records (metadata + embeddings) so the Rust
# test can reconstruct identical inputs rather than trying to replicate
# numpy's RNG.
inputs = []
for p in photos:
    meta = {k: v for k, v in p.items() if k not in ("embedding_seed",) and v is not None}
    inputs.append({
        "photo_id": p["photo_id"],
        "filename": p["filename"],
        "capture_time": p["capture_time"],
        "phash": p["cull_phash"],
        "embedding": embedding(p["embedding_seed"]),
        "metadata": meta,
    })
with open("/Users/bm/src/LrGeniusAI/server-rs/testdata/grouping_goldens_inputs.json", "w") as f:
    json.dump(inputs, f)
print("wrote inputs")
