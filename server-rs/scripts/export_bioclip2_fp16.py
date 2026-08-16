"""
Export BioCLIP 2 (ViT-L/14, `imageomics/bioclip-2`) for the Rust `ort`
runtime: the image tower as fp16 ONNX, plus a *pruned* zero-shot taxonomy
head repacked into a compact binary the backend can mmap and score against.

BioCLIP does not classify by itself — it is a CLIP. The taxonomy lives in a
precomputed text-embedding matrix over TreeOfLife-200M's 867,455 taxa, and
`lrg-ml::bioclip::classify` turns image-vs-taxa cosine into per-rank
predictions by summing softmax mass over taxonomic prefixes. So there are two
artifacts here, and the head is the bigger of the two upstream:

    open_clip_model.safetensors      1.71 GB fp32  (both towers)
    txt_emb_bioclip-2.npy            2.66 GB fp32  ([768, 867455], dim-major)
    txt_emb_bioclip-2.json             92 MB       (7 ranks + common name/row)

We ship only the image tower (the text tower is never run at inference time —
the head is precomputed), and we prune the head per
`bioclip_taxa_filter.toml`. See that file for the rules and, more
importantly, for what pruning costs.

Run once per release:
    cd server-rs
    uv run --project scripts --with onnxscript \
        python scripts/export_bioclip2_fp16.py --output-dir /tmp/bioclip2-export

(`--with onnxscript`: torch >=2.9's `torch.onnx.export` imports it
unconditionally at call time even with `dynamo=False` — a throwaway overlay
for this one script, same as export_siglip2_fp16.py.)

Then verify without re-exporting (needs onnxruntime/numpy only):
    uv run --project scripts python scripts/export_bioclip2_fp16.py \
        --verify-only --output-dir /tmp/bioclip2-export

Outputs:
    bioclip2_image_fp16.onnx   image tower, fp16 weights / fp32 I/O
    bioclip2_taxa.bin          header + fp16 [N, 768] row-major, L2-normalized
    bioclip2_taxa.json         interned labels (see write_taxa_labels)
"""

from __future__ import annotations

import argparse
import faulthandler
import json
import struct
import sys
import tomllib
from pathlib import Path

import numpy as np

# A bare "Process completed with exit code 139" (SIGSEGV) with no traceback is
# otherwise all CI gives us for a native crash in torch/onnxruntime.
faulthandler.enable()
sys.stdout.reconfigure(line_buffering=True)

MODEL_STR = "hf-hub:imageomics/bioclip-2"
TOL_DATAFILE_REPO = "imageomics/TreeOfLife-200M"
TXT_EMB_NPY = "embeddings/txt_emb_bioclip-2.npy"
TXT_EMB_JSON = "embeddings/txt_emb_bioclip-2.json"

IMAGE_SIZE = 224
EMBED_DIM = 768
OPSET = 18

# Bumped whenever the *contents* of the head change in a way that invalidates
# already-stored per-photo predictions. The backend stamps this into each
# photo's `species_model` metadata so `check_unprocessed` can re-queue them.
TAXA_VERSION = "taxa-v1"

RANKS = ["kingdom", "phylum", "class", "order", "family", "genus", "species"]

# `bioclip2_taxa.bin` header: little-endian, 28 bytes, then the fp16 matrix
# row-major. Kept byte-for-byte in sync with `lrg-ml::bioclip::TaxaHead::load`.
TAXA_MAGIC = b"LRGTAXA\0"
TAXA_FORMAT_VERSION = 1
TAXA_HEADER_STRUCT = "<8sIIIfI"  # magic, version, n_rows, dim, logit_scale_exp, pad


def load_model():
    import open_clip

    model, preprocess = open_clip.create_model_from_pretrained(
        MODEL_STR, return_transform=True
    )
    model.eval()
    return model, preprocess


def export_image_tower(model, output_dir: Path) -> Path:
    # Imported lazily so `--verify-only` needs neither torch nor open_clip.
    import torch

    class ImageTower(torch.nn.Module):
        """fp32 in -> fp16 compute -> fp32 out, so the traced graph is
        internally fp16 with an fp32 boundary. Tracing natively in fp16 rather
        than post-hoc graph conversion — see export_siglip2_fp16.py for the
        two onnxconverter_common failures that motivated this."""

        def __init__(self, model):
            super().__init__()
            self.model = model

        def forward(self, pixel_values):
            return self.model.encode_image(pixel_values.half()).float()

    output_dir.mkdir(parents=True, exist_ok=True)
    image_path = output_dir / "bioclip2_image_fp16.onnx"
    half_model = model.half()

    dummy_pixels = torch.zeros(1, 3, IMAGE_SIZE, IMAGE_SIZE, dtype=torch.float32)
    torch.onnx.export(
        ImageTower(half_model),
        (dummy_pixels,),
        str(image_path),
        input_names=["pixel_values"],
        output_names=["image_embeds"],
        dynamic_axes={"pixel_values": {0: "batch"}, "image_embeds": {0: "batch"}},
        opset_version=OPSET,
        dynamo=False,
    )
    print(
        f"Exported fp16 image tower -> {image_path} "
        f"({image_path.stat().st_size / 1e6:.1f} MB)"
    )
    return image_path


def download_head() -> tuple[Path, Path]:
    from huggingface_hub import hf_hub_download

    print(f"Fetching taxonomy head from {TOL_DATAFILE_REPO} (2.7 GB, cached)...")
    npy = hf_hub_download(
        repo_id=TOL_DATAFILE_REPO, filename=TXT_EMB_NPY, repo_type="dataset"
    )
    labels = hf_hub_download(
        repo_id=TOL_DATAFILE_REPO, filename=TXT_EMB_JSON, repo_type="dataset"
    )
    return Path(npy), Path(labels)


def select_rows(names: list, rules: dict) -> list[int]:
    """Apply bioclip_taxa_filter.toml to the upstream label list.

    `names` is the upstream shape: one entry per taxon, each
    `[[kingdom, ..., species_epithet], common_name]`.
    """
    whitelist = set(rules["class_whitelist"])
    require_common = rules.get("require_common_name", True)
    keep_genus = rules.get("keep_one_per_genus", True)

    class_idx = RANKS.index("class")
    genus_idx = RANKS.index("genus")

    kept: list[int] = []
    # Genera that already have a kept row, so the second pass only has to fill
    # the gaps rather than re-scan.
    covered_genera: set[tuple[str, ...]] = set()
    in_whitelist: list[int] = []

    for i, entry in enumerate(names):
        path, common = entry[0], entry[1]
        if len(path) <= class_idx or path[class_idx] not in whitelist:
            continue
        in_whitelist.append(i)
        if require_common and not common:
            continue
        kept.append(i)
        covered_genera.add(tuple(path[: genus_idx + 1]))

    if keep_genus:
        # One representative per genus that the common-name rule dropped
        # entirely. Deterministic: the first row in upstream order.
        for i in in_whitelist:
            genus_key = tuple(names[i][0][: genus_idx + 1])
            if genus_key in covered_genera:
                continue
            kept.append(i)
            covered_genera.add(genus_key)
        kept.sort()

    max_rows = rules.get("max_rows")
    if max_rows and len(kept) > max_rows:
        raise SystemExit(
            f"Taxa filter kept {len(kept)} rows, over the max_rows={max_rows} "
            f"ceiling in bioclip_taxa_filter.toml. Tighten the rules or raise "
            f"the ceiling deliberately."
        )

    print(
        f"  taxa: {len(names)} upstream -> {len(in_whitelist)} in whitelisted "
        f"classes -> {len(kept)} kept ({len(covered_genera)} genera)"
    )
    return kept


def write_taxa_matrix(
    npy_path: Path, keep: list[int], logit_scale_exp: float, dest: Path
) -> None:
    """Slice, transpose, normalize and pack the head.

    Upstream is `[dim, N]` (dim-major — pybioclip does `img @ txt` with
    `img` as `[B, dim]`, and its `apply_filter` slices `txt[:, idx]`). We
    write `[N, dim]` row-major instead: the backend scores one image against
    every taxon, so a row-major matrix keeps that matvec sequential in cache.
    """
    mat = np.load(npy_path, mmap_mode="r")
    if mat.shape[0] != EMBED_DIM:
        raise SystemExit(
            f"Expected upstream head shaped [{EMBED_DIM}, N], got {mat.shape}"
        )
    print(f"  upstream head: {mat.shape} {mat.dtype}")

    rows = np.ascontiguousarray(mat[:, keep].T.astype(np.float32))
    # Defensive: upstream should already be unit-norm, but the whole scoring
    # path assumes it, so make it true rather than trusting it.
    norms = np.linalg.norm(rows, axis=1, keepdims=True)
    max_drift = float(np.abs(norms - 1.0).max())
    if max_drift > 1e-3:
        print(f"  note: upstream rows were not unit-norm (max drift {max_drift:.2e})")
    rows /= np.maximum(norms, 1e-12)

    header = struct.pack(
        TAXA_HEADER_STRUCT,
        TAXA_MAGIC,
        TAXA_FORMAT_VERSION,
        len(keep),
        EMBED_DIM,
        logit_scale_exp,
        0,
    )
    with dest.open("wb") as fh:
        fh.write(header)
        fh.write(rows.astype(np.float16).tobytes())
    print(f"Wrote taxa matrix -> {dest} ({dest.stat().st_size / 1e6:.1f} MB)")


def write_taxa_labels(names: list, keep: list[int], dest: Path) -> None:
    """Write the kept labels with per-rank string interning.

    The upstream JSON repeats every ancestor name on every row — "Animalia"
    appears hundreds of thousands of times. Interning per rank shrinks the
    file by ~4x, but the real reason is the backend: `classify` aggregates
    probabilities by taxonomic prefix on every single photo, and comparing
    interned `u32` ids beats comparing strings by a wide margin.
    """
    vocabs: list[dict[str, int]] = [{} for _ in RANKS]
    rows: list[list[int]] = []
    commons: list[str] = []

    for i in keep:
        path, common = names[i][0], names[i][1]
        ids = []
        for rank_i in range(len(RANKS)):
            value = path[rank_i] if rank_i < len(path) else ""
            vocab = vocabs[rank_i]
            if value not in vocab:
                vocab[value] = len(vocab)
            ids.append(vocab[value])
        rows.append(ids)
        commons.append(common or "")

    payload = {
        "version": TAXA_FORMAT_VERSION,
        "model": "bioclip-2",
        "taxa_version": TAXA_VERSION,
        "ranks": RANKS,
        # The species rank holds the *epithet* only, as upstream does; the
        # binomial is genus + " " + epithet, assembled by the backend.
        "vocab": [sorted(v, key=v.get) for v in vocabs],
        "rows": rows,
        "common": commons,
    }
    dest.write_text(json.dumps(payload, separators=(",", ":"), ensure_ascii=False))
    print(f"Wrote taxa labels -> {dest} ({dest.stat().st_size / 1e6:.1f} MB)")


def export_head(model, output_dir: Path, rules: dict) -> None:
    npy_path, labels_path = download_head()
    names = json.loads(labels_path.read_text(encoding="utf-8"))
    keep = select_rows(names, rules)

    logit_scale_exp = float(model.logit_scale.exp().detach().float())
    print(f"  logit_scale.exp() = {logit_scale_exp:.4f}")

    write_taxa_matrix(
        npy_path, keep, logit_scale_exp, output_dir / "bioclip2_taxa.bin"
    )
    write_taxa_labels(names, keep, output_dir / "bioclip2_taxa.json")


# --- verification -----------------------------------------------------------


def _lcg_bytes(n, seed=42):
    """Must match testdata/make_bioclip_goldens.py exactly — the goldens store
    embeddings, not pixels."""
    out = np.empty(n, dtype=np.uint8)
    x = seed
    for i in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        out[i] = (x >> 16) & 0xFF
    return out


def _gradient(w, h):
    n = w * h * 3
    vals = (np.arange(n, dtype=np.float64) * 255.0 / (n - 1)).astype(np.uint8)
    return vals.reshape(h, w, 3)


def _make_golden_image(name: str) -> np.ndarray:
    if name == "gradient_640x480":
        return _gradient(640, 480)
    if name == "lcg_noise_800x600":
        return _lcg_bytes(800 * 600 * 3).reshape(600, 800, 3)
    if name == "small_320x240":
        return _lcg_bytes(320 * 240 * 3, seed=99).reshape(240, 320, 3)
    raise ValueError(f"unknown golden image name: {name}")


def load_taxa_matrix(path: Path) -> tuple[np.ndarray, float]:
    with path.open("rb") as fh:
        header = fh.read(struct.calcsize(TAXA_HEADER_STRUCT))
        magic, version, n_rows, dim, logit_scale_exp, _ = struct.unpack(
            TAXA_HEADER_STRUCT, header
        )
        if magic != TAXA_MAGIC:
            raise SystemExit(f"{path}: bad magic {magic!r}")
        if version != TAXA_FORMAT_VERSION:
            raise SystemExit(f"{path}: unsupported format version {version}")
        raw = np.frombuffer(fh.read(), dtype=np.float16)
    return raw.reshape(n_rows, dim).astype(np.float32), logit_scale_exp


def verify_embeddings(output_dir: Path, goldens_path: Path, preprocess) -> bool:
    """Cosine of the fp16 ONNX image tower against fp32 torch goldens."""
    import onnxruntime as ort

    # Default thread auto-detection segfaults on cgroup-restricted hosts such
    # as GitHub Actions runners (microsoft/onnxruntime#7207).
    session_options = ort.SessionOptions()
    session_options.intra_op_num_threads = 1
    session_options.inter_op_num_threads = 1

    if not goldens_path.exists():
        print(f"  {goldens_path} not found; skipping embedding verification")
        return True

    goldens = json.loads(goldens_path.read_text())
    sess = ort.InferenceSession(
        str(output_dir / "bioclip2_image_fp16.onnx"),
        sess_options=session_options,
        providers=["CPUExecutionProvider"],
    )

    if preprocess is None:
        print(
            "  (image goldens skipped in --verify-only mode — they need "
            "open_clip's preprocess transform, not just the ONNX graph)"
        )
        return True

    from PIL import Image

    ok = True
    print("\n=== fp16 ONNX vs fp32 torch goldens ===")
    for name, entry in goldens["images"].items():
        pil = Image.fromarray(_make_golden_image(name), "RGB")
        tensor = preprocess(pil).unsqueeze(0).numpy().astype(np.float32)
        (emb,) = sess.run(["image_embeds"], {"pixel_values": tensor})
        golden = np.array(entry["embedding"], dtype=np.float32)
        cos = float(
            np.dot(emb[0], golden) / (np.linalg.norm(emb[0]) * np.linalg.norm(golden))
        )
        status = "OK" if cos > 0.999 else "FAIL"
        ok = ok and cos > 0.999
        print(f"  image '{name}': cosine={cos:.6f} [{status}]")
    return ok


def verify_prune(output_dir: Path, fixtures: Path | None) -> bool:
    """Compare the pruned head's top-1 against the *full* upstream head.

    This is the measurement that justifies (or kills) the pruning rules. A
    pruned head never says "I don't know" — it redistributes the missing
    clade's probability mass onto surviving rows — so the only honest check
    is how often it still lands on the same taxon as the full head.
    """
    if fixtures is None or not fixtures.is_dir():
        print(
            "\n  (prune check skipped: pass --fixtures <dir of organism photos> "
            "to compare pruned vs full head)"
        )
        return True

    import onnxruntime as ort
    from PIL import Image

    session_options = ort.SessionOptions()
    session_options.intra_op_num_threads = 1
    session_options.inter_op_num_threads = 1
    sess = ort.InferenceSession(
        str(output_dir / "bioclip2_image_fp16.onnx"),
        sess_options=session_options,
        providers=["CPUExecutionProvider"],
    )

    pruned, _ = load_taxa_matrix(output_dir / "bioclip2_taxa.bin")
    labels = json.loads((output_dir / "bioclip2_taxa.json").read_text())
    pruned_names = [
        " ".join(labels["vocab"][r][row[r]] for r in range(len(RANKS)))
        for row in labels["rows"]
    ]

    npy_path, full_labels_path = download_head()
    full = np.load(npy_path, mmap_mode="r")
    full_names_raw = json.loads(full_labels_path.read_text(encoding="utf-8"))

    images = sorted(
        p
        for p in fixtures.iterdir()
        if p.suffix.lower() in {".jpg", ".jpeg", ".png", ".tif", ".tiff"}
    )
    if not images:
        print(f"  {fixtures} holds no images; skipping prune check")
        return True

    print(f"\n=== pruned head vs full head, top-1 over {len(images)} fixtures ===")
    agree = 0
    for path in images:
        pil = Image.open(path).convert("RGB")
        # Mirrors lrg-ml::bioclip_pre and pybioclip: squash to 224x224
        # (no center crop), OpenAI-CLIP mean/std.
        arr = np.asarray(pil.resize((IMAGE_SIZE, IMAGE_SIZE), Image.BILINEAR))
        arr = arr.astype(np.float32) / 255.0
        mean = np.array([0.48145466, 0.4578275, 0.40821073], dtype=np.float32)
        std = np.array([0.26862954, 0.26130258, 0.27577711], dtype=np.float32)
        tensor = ((arr - mean) / std).transpose(2, 0, 1)[None].astype(np.float32)

        (emb,) = sess.run(["image_embeds"], {"pixel_values": tensor})
        vec = emb[0] / np.linalg.norm(emb[0])

        pruned_top = pruned_names[int(np.argmax(pruned @ vec))]
        full_top_idx = int(np.argmax(vec @ np.asarray(full, dtype=np.float32)))
        full_top = " ".join(full_names_raw[full_top_idx][0])

        same = pruned_top == full_top
        agree += same
        print(f"  {path.name}: {'==' if same else '!='} pruned={pruned_top!r}")
        if not same:
            print(f"      full={full_top!r}")

    rate = agree / len(images)
    print(f"  top-1 agreement: {agree}/{len(images)} ({rate:.0%})")
    if rate < 0.8:
        print(
            "  WARNING: agreement below 80%. The pruning rules are dropping taxa "
            "these fixtures actually need — widen the whitelist, or raise the "
            "per-rank confidence floor so the backend reports a higher rank "
            "instead of a confidently wrong species.",
            file=sys.stderr,
        )
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "--verify-only",
        action="store_true",
        help="Skip export, only verify existing output-dir contents",
    )
    parser.add_argument(
        "--fixtures",
        type=Path,
        default=None,
        help="Directory of real organism photos for the pruned-vs-full top-1 check",
    )
    parser.add_argument(
        "--rules",
        type=Path,
        default=Path(__file__).resolve().parent / "bioclip_taxa_filter.toml",
    )
    parser.add_argument(
        "--goldens",
        type=Path,
        default=Path(__file__).resolve().parent.parent
        / "testdata"
        / "bioclip_goldens.json",
    )
    args = parser.parse_args()

    preprocess = None
    if not args.verify_only:
        rules = tomllib.loads(args.rules.read_text())["filter"]
        model, preprocess = load_model()
        args.output_dir.mkdir(parents=True, exist_ok=True)
        export_head(model, args.output_dir, rules)
        export_image_tower(model, args.output_dir)

    total = sum(
        (args.output_dir / n).stat().st_size
        for n in ("bioclip2_image_fp16.onnx", "bioclip2_taxa.bin", "bioclip2_taxa.json")
        if (args.output_dir / n).exists()
    )
    print(f"\nTotal distribution size: {total / 1e6:.1f} MB")

    ok = verify_embeddings(args.output_dir, args.goldens, preprocess)
    verify_prune(args.output_dir, args.fixtures)

    if not ok:
        print(
            "\nVerification FAILED — fp16 conversion degraded the image tower "
            "beyond tolerance.",
            file=sys.stderr,
        )
        sys.exit(1)
    print("\nVerification passed.")


if __name__ == "__main__":
    main()
