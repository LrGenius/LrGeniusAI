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
import re
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
#
# v2: dropped rows with an unusable genus or species epithet and repaired the
# 1.73% whose `species` field was a whole binomial or carried a taxonomic
# authority. Any photo that had been classified onto one of those rows holds a
# wrong answer, and there is no way to tell which from the stored result alone.
TAXA_VERSION = "taxa-v2"

RANKS = ["kingdom", "phylum", "class", "order", "family", "genus", "species"]

# `bioclip2_taxa.bin` header: little-endian, 28 bytes, then the fp16 matrix
# row-major. Kept byte-for-byte in sync with `lrg-ml::bioclip::TaxaHead::load`.
TAXA_MAGIC = b"LRGTAXA\0"
TAXA_FORMAT_VERSION = 1
TAXA_HEADER_STRUCT = "<8sIIIfI"  # magic, version, n_rows, dim, logit_scale_exp, pad


def bioclip_preprocess():
    """The transform BioCLIP 2 is actually used with.

    **Not** `open_clip`'s own transform for this checkpoint. pybioclip
    deliberately overrides it for TreeOfLife models
    (`BaseClassifier.load_pretrained_model`: `self.preprocess = preprocess_img
    if self.model_str in TOL_MODELS else preprocess`), and it is the override
    the weights are used with in practice — squash to 224x224 rather than
    resize-and-crop.

    Getting this wrong is not loud. An earlier version of this script verified
    against `open_clip`'s transform while `testdata/make_bioclip_goldens.py`
    generated the goldens through pybioclip's, and the mismatch showed up as
    cosines of 0.97-0.99 that read like fp16 damage to the exported graph. The
    graph was fine: `lrg-ml`'s own golden test scored 0.999998 against the same
    file. Both sides now come from this one function.
    """
    from torchvision import transforms

    return transforms.Compose(
        [
            transforms.ToTensor(),
            transforms.Resize((IMAGE_SIZE, IMAGE_SIZE), antialias=True),
            transforms.Normalize(
                mean=(0.48145466, 0.4578275, 0.40821073),
                std=(0.26862954, 0.26130258, 0.27577711),
            ),
        ]
    )


def load_model():
    import open_clip

    model = open_clip.create_model(MODEL_STR)
    model.eval()
    return model


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


# A valid genus is one capitalised word; a valid species epithet is one
# lowercase word, hyphens allowed (`x-notata` is a real epithet).
GENUS_RE = re.compile(r"^[A-Z][a-z-]+$")
EPITHET_RE = re.compile(r"^[a-z][a-z-]*$")
# Open nomenclature: these mark "not identified to species", so a row carrying
# one is not an identification and has no business in a classifier's head.
OPEN_NOMENCLATURE = {"sp", "spp", "cf", "aff", "indet", "nr", "var", "subsp"}


def clean_epithet(species: str) -> str | None:
    """Reduce a raw `species` field to a bare epithet, or `None` to drop it.

    1.73% of the head's rows have a `species` field that is not a bare epithet,
    and they are wrong in two opposite directions:

        'alpinum Hudson ex Withering, 1801'   epithet first, then the authority
        'Trigonotarbus schucherti'            a whole binomial, the *original*
                                              combination, while `genus` holds
                                              the currently accepted name

    Taking the first token would break the second case and taking the last
    would break the first. What both have in common is that the epithet is the
    first token that looks like one: a bare lowercase word. Authorities are
    capitalised or carry digits and commas, original genera are capitalised,
    and `sp.`/`cf.` carry a dot — so none of them can be mistaken for it.

    Without this the backend emits 'Lissomartus Trigonotarbus schucherti' as a
    binomial, and writes it into a keyword.
    """
    for token in species.split():
        if token.rstrip(".").lower() in OPEN_NOMENCLATURE:
            # An open-nomenclature marker means everything after it is a
            # specimen code, not a name. Nothing usable is left.
            return None
        if EPITHET_RE.match(token):
            return token
    return None


def clean_rows(names: list, keep: list[int]) -> tuple[list[int], dict[int, str]]:
    """Drop unusable rows and repair salvageable epithets.

    Returns the surviving row indices plus the epithets to substitute, rather
    than mutating `names`: the caller also uses those indices to slice the
    embedding matrix, so the two must not drift apart.
    """
    survivors: list[int] = []
    repaired: dict[int, str] = {}
    dropped_genus = dropped_species = 0
    for i in keep:
        path = names[i][0]
        if not GENUS_RE.match(path[5]):
            # 'unclassified Cecidomyiinae', 'Sogdini unplaced', 'Calvittacus ue'
            dropped_genus += 1
            continue
        epithet = clean_epithet(path[6])
        if epithet is None:
            dropped_species += 1
            continue
        if epithet != path[6]:
            repaired[i] = epithet
        survivors.append(i)
    print(
        f"  names: dropped {dropped_genus} rows on an unusable genus and "
        f"{dropped_species} on an unusable species epithet, "
        f"repaired {len(repaired)} epithets"
    )
    return survivors, repaired


def live_mask(npy_path: Path) -> np.ndarray:
    """Which upstream taxa actually have an embedding.

    1.79% of TreeOfLife-200M's taxa (15,487 of 867,455) are listed in the label
    file with an all-zero column in the matrix — an upstream data defect, not
    something this export causes. They cannot be excluded by any taxonomic
    rule, so they get their own pass.

    Dropping them matters more than 1.79% suggests, because they are not evenly
    spread: they take out nearly every fern and conifer. Keeping them would
    ship rows that can never be predicted while their *nodes* still count in
    the per-rank aggregation, so a fern photo would have its probability
    pushed onto a neighbouring clade instead of onto Polypodiopsida.

    Computed in column blocks — the memmap is 2.66 GB and materializing it
    whole to take a norm costs several GB of peak RSS for no reason.
    """
    mat = np.load(npy_path, mmap_mode="r")
    n = mat.shape[1]
    live = np.empty(n, dtype=bool)
    block = 50_000
    for start in range(0, n, block):
        chunk = np.asarray(mat[:, start : start + block], dtype=np.float32)
        live[start : start + block] = np.einsum("ij,ij->j", chunk, chunk) > 1e-12
    dead = int((~live).sum())
    print(f"  embeddings: {dead} of {n} upstream taxa have no embedding, dropped")
    return live


def select_rows(names: list, rules: dict, live: np.ndarray) -> list[int]:
    """Apply bioclip_taxa_filter.toml to the upstream label list.

    `names` is the upstream shape: one entry per taxon, each
    `[[kingdom, ..., species_epithet], common_name]`. `live` masks out taxa
    with no embedding (see `live_mask`) *before* any rule runs, so
    `keep_one_per_genus` picks a representative that can actually be matched.
    """
    whitelist = set(rules["class_whitelist"])
    require_common = rules.get("require_common_name", True)
    keep_genus = rules.get("keep_one_per_genus", True)
    keep_fish = rules.get("keep_chordata_without_class", False)

    phylum_idx = RANKS.index("phylum")
    class_idx = RANKS.index("class")
    genus_idx = RANKS.index("genus")

    kept: list[int] = []
    # Genera that already have a kept row, so the second pass only has to fill
    # the gaps rather than re-scan.
    covered_genera: set[tuple[str, ...]] = set()
    in_whitelist: list[int] = []

    for i, entry in enumerate(names):
        path, common = entry[0], entry[1]
        if len(path) <= class_idx or not live[i]:
            continue
        selected = path[class_idx] in whitelist or (
            keep_fish and not path[class_idx] and path[phylum_idx] == "Chordata"
        )
        if not selected:
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

    # Printed before the ceiling check on purpose: when the export fails here,
    # the breakdown is exactly what you need to decide which rule to change.
    with_common = sum(1 for i in kept if names[i][1])
    print(
        f"  taxa: {len(names)} upstream -> {len(in_whitelist)} in whitelisted "
        f"classes -> {len(kept)} kept "
        f"({with_common} with a common name, {len(kept) - with_common} genus "
        f"representatives, {len(covered_genera)} genera)"
    )
    # Resident cost equals the on-disk cost: `lrg-ml::bioclip` keeps the head
    # at fp16 and scores through a lookup table rather than dequantizing.
    print(f"  head: {len(kept) * EMBED_DIM * 2 / 1e6:.0f} MB on disk and resident")

    max_rows = rules.get("max_rows")
    if max_rows and len(kept) > max_rows:
        raise SystemExit(
            f"Taxa filter kept {len(kept)} rows, over the max_rows={max_rows} "
            f"ceiling in bioclip_taxa_filter.toml. Tighten the rules or raise "
            f"the ceiling deliberately."
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
        # `live_mask` already removed the all-zero columns, so anything left
        # off the unit sphere is unexpected and worth looking at.
        print(f"  WARNING: rows off the unit sphere after filtering (max drift {max_drift:.2e})")
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


def write_taxa_labels(
    names: list, keep: list[int], repaired: dict[int, str], dest: Path
) -> None:
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
            if rank_i == 6 and i in repaired:
                value = repaired[i]
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
    keep = select_rows(names, rules, live_mask(npy_path))
    keep, repaired = clean_rows(names, keep)

    logit_scale_exp = float(model.logit_scale.exp().detach().float())
    print(f"  logit_scale.exp() = {logit_scale_exp:.4f}")

    write_taxa_matrix(
        npy_path, keep, logit_scale_exp, output_dir / "bioclip2_taxa.bin"
    )
    write_taxa_labels(names, keep, repaired, output_dir / "bioclip2_taxa.json")


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
        "--skip-tower",
        action="store_true",
        help="Re-export only the taxonomy head, reusing the existing ONNX tower. "
        "The tower takes minutes and only changes when the checkpoint does, "
        "while the head changes whenever the taxa rules do.",
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

    preprocess = bioclip_preprocess()
    if not args.verify_only:
        rules = tomllib.loads(args.rules.read_text())["filter"]
        model = load_model()
        args.output_dir.mkdir(parents=True, exist_ok=True)
        export_head(model, args.output_dir, rules)
        if args.skip_tower:
            print("Skipping the image tower export (--skip-tower); reusing the existing one")
        else:
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
