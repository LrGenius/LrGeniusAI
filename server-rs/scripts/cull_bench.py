#!/usr/bin/env python3
"""Public culling benchmark: free-licence photographs from Wikimedia Commons,
with the defects culling has to judge made on purpose, so the right answer is
known without anyone culling by hand.

Every source photograph becomes one series, as if shot in a burst: the photo
itself, and copies of it with one defect each (focus missed, focus on the
background instead of the subject, eyes out of focus, camera shake, a stop too
dark…), and copies that look soft but must survive any cull because the subject
is sharp (shallow depth of field, a panning shot). The fixture says which frames
may win, which must go, which must stay, and what the explanation should and
should not say. `cull_eval` scores it:

    cargo run --release -p lrg-analysis --example cull_eval -- \\
        testdata/cull_bench/fixtures --by-defect

What it measures is whether culling tells these defects apart, per genre. It
does not measure taste: which of two equally sharp frames holds the better
moment is nothing a made-up defect can answer. That still takes a hand-culled
shoot (*Export Culling Fixture* in Lightroom).

Steps, from server-rs/scripts:

    uv run python cull_bench.py select --per-genre 24
        Picks photos per genre from Commons quality-image categories, checks
        each one's licence and content (faces for portraits and events, an
        animal for wildlife…), and writes testdata/cull_bench/sources.toml.
    uv run python cull_bench.py make
        Downloads them and makes the series. Images stay in the cache.
    uv run python cull_bench.py run --url http://127.0.0.1:19830
        Indexes the series on a backend of this build and writes one fixture
        per genre to testdata/cull_bench/fixtures/.

`run` wants a backend on a database of its own, never the one a catalog uses:

    GENIUSAI_PORT=19830 cargo run --release -p lrg-server -- \\
        --db-path ~/.cache/lrgenius/cull-bench/db/lrgenius.db

Images are cached under ~/.cache/lrgenius/cull-bench (LRG_CULL_BENCH_CACHE moves
it). Only the list of sources, with each photo's licence and author, and the
fixtures (measurements, no pixels) are in the repository.

The subject masks and eye positions that place the defects come from
torchvision's Mask R-CNN and Keypoint R-CNN, not from the models the backend
ships: a benchmark whose answers came from the model under test would only
measure that model's agreement with itself.
"""

from __future__ import annotations

import argparse
import hashlib
import html
import io
import json
import math
import os
import re
import sys
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

SERVER_RS = Path(__file__).resolve().parents[1]
BENCH_DIR = SERVER_RS / "testdata" / "cull_bench"
SOURCES_TOML = BENCH_DIR / "sources.toml"
FIXTURES_DIR = BENCH_DIR / "fixtures"
CACHE = Path(
    os.environ.get(
        "LRG_CULL_BENCH_CACHE", Path.home() / ".cache" / "lrgenius" / "cull-bench"
    )
)

API = "https://commons.wikimedia.org/w/api.php"
USER_AGENT = "LrGeniusAI-cull-bench/1.0 (https://github.com/LrGenius/LrGeniusAI)"

# Every series is made at this long edge. Large enough that an eye in a
# half-length portrait has the 40 px between the eyes the backend's eye focus
# needs, small enough that a run over a thousand frames takes an hour, not a day.
LONG_EDGE = 3000

# Licences a photograph may carry. Commons only hosts free files, but a few
# carry terms (GFDL-only, "attribution" templates) that are awkward to credit
# in a table, so the list is narrow on purpose.
LICENCE_RE = re.compile(r"^(CC0|Public domain|PD|CC BY(-SA)? \d(\.\d)?)", re.I)

# COCO labels the subject can be (torchvision's numbering).
PERSON = 1
ANIMALS = set(range(16, 26))  # bird, cat, dog, horse, sheep, cow, elephant, bear, zebra, giraffe
VEHICLES = set(range(2, 10))  # bicycle, car, motorcycle, airplane, bus, train, truck, boat


@dataclass(frozen=True)
class Genre:
    name: str
    preset: str
    categories: tuple[str, ...]
    depth: int


# Event comes before portrait: both draw on photographs of people, and a photo
# of several people with their eyes visible is more useful as an event series.
GENRES = (
    Genre(
        "event",
        "event",
        (
            "Quality images of people",
            "Quality images of events",
            "Quality images of concerts",
            "Quality images of children",
        ),
        1,
    ),
    Genre(
        "portrait",
        "portrait",
        ("Quality images of people", "Quality images of children"),
        0,
    ),
    Genre("sport", "sports", ("Quality images of sports",), 2),
    Genre(
        "wildlife",
        "wildlife",
        (
            "Quality images of birds",
            "Quality images of mammals",
            "Quality images of animals",
        ),
        2,
    ),
    Genre("landscape", "default", ("Quality images of landscapes",), 0),
)
GENRE_BY_NAME = {g.name: g for g in GENRES}


@dataclass(frozen=True)
class Variant:
    """One frame of a series, and what the right answer about it is."""

    name: str
    # What the photograph must have for this defect to be made: "eyes" (a
    # person whose eyes are large enough to judge), "subject" (a subject mask),
    # "panning" (a subject in a genre where panning is a technique).
    needs: str | None
    # 1 = may win the series, 2 = keepable but worse, 3 = should go.
    rank: int
    reject: bool = False
    reject_optional: bool = False
    must_keep: bool = False
    expect: tuple[str, ...] = ()
    forbid: tuple[str, ...] = ()


FOCUS_REASONS = ("blurred", "motion_blur", "eyes_out_of_focus", "face_out_of_focus")

VARIANTS = (
    Variant("original", None, 1, forbid=FOCUS_REASONS),
    # Visible at 100 %, not at screen size: a photographer would take the
    # sharp twin, and would not call this one a defect.
    Variant("blur_mild", None, 2, reject_optional=True),
    Variant("blur_strong", None, 3, reject=True, expect=("blurred", "motion_blur")),
    Variant("motion", None, 3, reject=True, expect=("motion_blur",)),
    Variant(
        "eyes_soft",
        "eyes",
        3,
        reject=True,
        expect=("eyes_out_of_focus", "face_out_of_focus"),
    ),
    Variant(
        "subject_soft",
        "subject",
        3,
        reject=True,
        expect=("subject_out_of_focus", "focus_on_background"),
    ),
    Variant("background_soft", "subject", 1, must_keep=True, forbid=FOCUS_REASONS),
    Variant(
        "panning", "panning", 1, must_keep=True, forbid=("blurred", "motion_blur")
    ),
    # Lightroom lifts a stop without a trace: worse than the twin, never a
    # defect on its own.
    Variant("under", None, 2, reject_optional=True, expect=("underexposed",)),
    Variant(
        "over",
        None,
        2,
        reject_optional=True,
        expect=("overexposed", "highlights_clipped"),
    ),
    Variant("noise", None, 2, reject_optional=True),
)

PANNING_GENRES = {"sport", "wildlife"}


# --- Commons -----------------------------------------------------------------


def _get_json(url: str) -> dict:
    for attempt in range(6):
        request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        try:
            with urllib.request.urlopen(request, timeout=90) as response:
                return json.load(response)
        except urllib.error.HTTPError as e:
            if e.code in (429, 500, 502, 503, 504) and attempt < 5:
                time.sleep(2 * 2**attempt)
                continue
            raise
        except urllib.error.URLError:
            if attempt < 5:
                time.sleep(2 * 2**attempt)
                continue
            raise
    raise RuntimeError(f"giving up on {url}")


def commons(params: dict) -> dict:
    query = {**params, "format": "json", "formatversion": "2"}
    return _get_json(API + "?" + urllib.parse.urlencode(query))


def category_files(category: str, depth: int) -> list[str]:
    """Every file title in a category and its subcategories down to `depth`."""
    seen_cats: set[str] = set()
    files: list[str] = []
    frontier = [("Category:" + category, 0)]
    while frontier:
        cat, level = frontier.pop(0)
        if cat in seen_cats:
            continue
        seen_cats.add(cat)
        cont: dict = {}
        while True:
            data = commons(
                {
                    "action": "query",
                    "list": "categorymembers",
                    "cmtitle": cat,
                    "cmtype": "file|subcat",
                    "cmlimit": "500",
                    **cont,
                }
            )
            for member in data.get("query", {}).get("categorymembers", []):
                if member["ns"] == 6:
                    files.append(member["title"])
                elif member["ns"] == 14 and level < depth:
                    frontier.append((member["title"], level + 1))
            if "continue" not in data:
                break
            cont = data["continue"]
    return files


def image_infos(titles: list[str]) -> dict[str, dict]:
    """Size, MIME type, URL and licence of each file, 50 per request."""
    out: dict[str, dict] = {}
    for i in range(0, len(titles), 50):
        batch = titles[i : i + 50]
        data = commons(
            {
                "action": "query",
                "titles": "|".join(batch),
                "prop": "imageinfo",
                "iiprop": "url|size|mime|extmetadata",
                "iiextmetadatafilter": "LicenseShortName|LicenseUrl|Artist",
            }
        )
        for page in data.get("query", {}).get("pages", []):
            info = (page.get("imageinfo") or [None])[0]
            if info:
                out[page["title"]] = info
    return out


def plain_text(markup: str) -> str:
    return " ".join(html.unescape(re.sub(r"<[^>]+>", " ", markup or "")).split())


def source_key(title: str) -> str:
    return hashlib.sha1(title.encode("utf-8")).hexdigest()[:10]


def download_base(source: dict) -> Image.Image:
    """The photograph at LONG_EDGE, cached. Commons serves a thumbnail at its
    next standard size above what is asked for, and the rest is a local resize."""
    path = CACHE / "originals" / f"{source['key']}.jpg"
    if path.exists():
        return Image.open(path).convert("RGB")
    w, h = int(source["width"]), int(source["height"])
    want_width = LONG_EDGE if w >= h else math.ceil(LONG_EDGE * w / h)
    data = commons(
        {
            "action": "query",
            "titles": source["title"],
            "prop": "imageinfo",
            "iiprop": "url",
            "iiurlwidth": str(want_width),
        }
    )
    info = data["query"]["pages"][0]["imageinfo"][0]
    url = info.get("thumburl") or info["url"]
    raw = b""
    for attempt in range(6):
        request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        try:
            with urllib.request.urlopen(request, timeout=180) as response:
                raw = response.read()
            break
        except urllib.error.HTTPError as e:
            if e.code in (429, 500, 502, 503, 504) and attempt < 5:
                time.sleep(3 * 2**attempt)
                continue
            raise
    image = Image.open(io.BytesIO(raw))
    image = image.convert("RGB")
    scale = LONG_EDGE / max(image.size)
    if abs(scale - 1.0) > 1e-3:
        size = (round(image.width * scale), round(image.height * scale))
        image = image.resize(size, Image.Resampling.LANCZOS)
    path.parent.mkdir(parents=True, exist_ok=True)
    image.save(path, quality=97, subsampling=0)
    time.sleep(0.5)  # be polite to the thumbnail servers
    return image


# --- content analysis (torchvision, independent of the shipped models) -------


class Detectors:
    def __init__(self) -> None:
        import torch
        from torchvision.models.detection import (
            KeypointRCNN_ResNet50_FPN_Weights,
            MaskRCNN_ResNet50_FPN_V2_Weights,
            keypointrcnn_resnet50_fpn,
            maskrcnn_resnet50_fpn_v2,
        )

        self.torch = torch
        # CPU: the detection heads are not reliably supported on MPS, and the
        # answers should not depend on which machine made them.
        self.device = torch.device("cpu")
        torch.set_num_threads(max(1, os.cpu_count() or 1))
        self.masks = maskrcnn_resnet50_fpn_v2(
            weights=MaskRCNN_ResNet50_FPN_V2_Weights.DEFAULT
        ).eval()
        self.keypoints = keypointrcnn_resnet50_fpn(
            weights=KeypointRCNN_ResNet50_FPN_Weights.DEFAULT
        ).eval()

    def analyse(self, image: Image.Image) -> dict:
        """People with their eye positions, and object masks, in base pixels."""
        torch = self.torch
        work_scale = 1333 / max(image.size)
        work = image.resize(
            (round(image.width * work_scale), round(image.height * work_scale)),
            Image.Resampling.BILINEAR,
        )
        tensor = (
            torch.from_numpy(np.asarray(work, dtype=np.float32) / 255.0)
            .permute(2, 0, 1)
            .contiguous()
        )
        with torch.inference_mode():
            m = self.masks([tensor])[0]
            k = self.keypoints([tensor])[0]
        inv = 1.0 / work_scale
        frame_area = work.width * work.height

        instances = []
        for i in range(len(m["scores"])):
            score = float(m["scores"][i])
            if score < 0.5:
                continue
            mask = (m["masks"][i, 0].numpy() >= 0.5).astype(np.uint8)
            area = float(mask.sum()) / frame_area
            if area <= 0:
                continue
            instances.append(
                {
                    "label": int(m["labels"][i]),
                    "score": score,
                    "box": [float(v) * inv for v in m["boxes"][i]],
                    "area": area,
                    "mask": mask,
                }
            )

        people = []
        for i in range(len(k["scores"])):
            score = float(k["scores"][i])
            if score < 0.9:
                continue
            kp = k["keypoints"][i].numpy()
            kps = k["keypoints_scores"][i].numpy()
            eyes = None
            iod = 0.0
            # COCO keypoints 0, 1 and 2 are the nose, the left and the right
            # eye. The model places eyes on the back of a head too, with weak
            # scores and in mirrored order (measured: nose and eyes 3-6 there,
            # 9-15 on faces turned to the camera). A face counts only when it
            # is turned to the camera: nose and both eyes sure, the person's
            # left eye on the right of the picture, the nose below the eyes.
            frontal = (
                min(kps[0], kps[1], kps[2]) >= 7.0
                and kp[1, 0] > kp[2, 0]
                and kp[0, 1] > min(kp[1, 1], kp[2, 1])
            )
            if frontal:
                eyes = [
                    [float(kp[1, 0]) * inv, float(kp[1, 1]) * inv],
                    [float(kp[2, 0]) * inv, float(kp[2, 1]) * inv],
                ]
                iod = math.dist(eyes[0], eyes[1])
            people.append(
                {
                    "score": score,
                    "box": [float(v) * inv for v in k["boxes"][i]],
                    "eyes": eyes,
                    "iod": iod,
                }
            )
        return {"size": list(image.size), "people": people, "instances": instances}


# Bump when `Detectors.analyse` changes what it finds, so cached analyses are
# made again. 2: eyes only on faces turned to the camera.
ANALYSIS_VERSION = 2


def analysis_path(key: str) -> Path:
    return CACHE / "analysis" / f"{key}.v{ANALYSIS_VERSION}.npz"


def save_analysis(key: str, analysis: dict) -> None:
    path = analysis_path(key)
    path.parent.mkdir(parents=True, exist_ok=True)
    masks = [inst.pop("mask") for inst in analysis["instances"]]
    meta = json.dumps(analysis)
    np.savez_compressed(
        path,
        meta=np.frombuffer(meta.encode(), dtype=np.uint8),
        **{f"mask{i}": m for i, m in enumerate(masks)},
    )
    for inst, m in zip(analysis["instances"], masks):
        inst["mask"] = m


def load_analysis(key: str) -> dict | None:
    path = analysis_path(key)
    if not path.exists():
        return None
    with np.load(path) as data:
        analysis = json.loads(bytes(data["meta"]).decode())
        for i, inst in enumerate(analysis["instances"]):
            inst["mask"] = data[f"mask{i}"]
    return analysis


def judged_people(analysis: dict) -> list[dict]:
    """People whose eyes are visible, largest first."""
    people = [p for p in analysis["people"] if p["eyes"]]
    return sorted(people, key=lambda p: -p["iod"])


def prominent_eyes(analysis: dict) -> list[dict]:
    """The people the backend counts for a frame: eyes visible, and at least a
    quarter as far apart as the largest pair (its own prominence gate is a
    quarter of the largest face's area)."""
    people = judged_people(analysis)
    if not people:
        return []
    top = people[0]["iod"]
    return [p for p in people if p["iod"] >= 0.5 * top]


def touching(a: np.ndarray, b: np.ndarray, grid: int = 78) -> bool:
    """Whether two masks overlap or lie next to each other, on a grid as coarse
    as the backend's: a rider's mask stops where the horse's starts."""

    def coarse(mask: np.ndarray) -> np.ndarray:
        h, w = mask.shape
        ys = np.linspace(0, h, grid + 1).astype(int)[:-1]
        xs = np.linspace(0, w, grid + 1).astype(int)[:-1]
        m = mask.astype(np.int32)
        return np.add.reduceat(np.add.reduceat(m, ys, axis=0), xs, axis=1) > 0

    ca, cb = coarse(a), coarse(b)
    padded = np.pad(ca, 1)
    grown = np.zeros_like(ca)
    for dy in range(3):
        for dx in range(3):
            grown |= padded[dy : dy + grid, dx : dx + grid]
    return bool((grown & cb).any())


def subject_instances(analysis: dict, genre: str) -> list[dict]:
    if genre == "wildlife":
        pool = [i for i in analysis["instances"] if i["label"] in ANIMALS]
    elif genre == "landscape":
        return []
    elif genre == "sport":
        # The athlete and what carries them: the horse under the rider, the
        # bicycle, the car. Blurring around the rider alone made the horse
        # "background", which no photographer would call it.
        persons = [
            i
            for i in analysis["instances"]
            if i["label"] == PERSON and i["score"] >= 0.8
        ]
        if not persons:
            return []
        largest = max(i["area"] for i in persons)
        athletes = [i for i in persons if i["area"] >= 0.25 * largest]
        body = np.zeros_like(athletes[0]["mask"], dtype=np.uint8)
        for inst in athletes:
            body |= inst["mask"].astype(np.uint8)
        mounts = [
            i
            for i in analysis["instances"]
            if (i["label"] in ANIMALS or i["label"] in VEHICLES)
            and i["score"] >= 0.6
            and i["area"] >= 0.25 * largest
            and touching(body, i["mask"])
        ]
        return athletes + mounts
    else:
        pool = [
            i
            for i in analysis["instances"]
            if i["label"] == PERSON and i["score"] >= 0.8
        ]
    if not pool:
        return []
    largest = max(i["area"] for i in pool)
    return [i for i in pool if i["area"] >= 0.25 * largest]


def ambiguous_subject(analysis: dict, genre: str) -> bool:
    """A person the detector is less sure of, but as large as one of the
    subject's, left out of it: blurring them is focus on one of two people, not
    a shallow depth of field, and which one the photo is of is anyone's guess.
    Such a photograph carries no subject defect."""
    subject = subject_instances(analysis, genre)
    people = [i for i in subject if i["label"] == PERSON]
    if not people:
        return False
    smallest = min(i["area"] for i in people)
    return any(
        i["label"] == PERSON
        and i["score"] >= 0.5
        and i["area"] >= smallest
        and not any(i is s for s in subject)
        for i in analysis["instances"]
    )


def subject_area(analysis: dict, genre: str) -> float:
    return sum(i["area"] for i in subject_instances(analysis, genre))


def accepts(analysis: dict, genre: str) -> tuple[bool, str]:
    """Whether a photograph can stand for its genre in the benchmark."""
    people = judged_people(analysis)
    area = subject_area(analysis, genre)
    persons = [
        i for i in analysis["instances"] if i["label"] == PERSON and i["score"] >= 0.8
    ]
    animals = [
        i for i in analysis["instances"] if i["label"] in ANIMALS and i["score"] >= 0.6
    ]
    if genre == "portrait":
        if not people or people[0]["iod"] < 55:
            return False, "no face large enough"
        if len(prominent_eyes(analysis)) > 2:
            return False, "a group, not a portrait"
        return True, f"face, eyes {people[0]['iod']:.0f} px apart"
    if genre == "event":
        judged = [p for p in people if p["iod"] >= 30]
        if len(judged) < 2:
            return False, "fewer than two faces large enough"
        return True, f"{len(judged)} faces"
    if genre == "sport":
        if not persons:
            return False, "no athlete"
        if not 0.015 <= area <= 0.6:
            return False, f"subject covers {area:.1%}"
        return True, f"athlete covers {area:.1%}"
    if genre == "wildlife":
        if not animals:
            return False, "no animal"
        if persons and max(p["area"] for p in persons) > max(
            a["area"] for a in animals
        ):
            return False, "people are the subject"
        if not 0.015 <= area <= 0.6:
            return False, f"subject covers {area:.1%}"
        return True, f"animal covers {area:.1%}"
    if genre == "landscape":
        if any(i["area"] > 0.01 for i in persons):
            return False, "people in it"
        if any(i["area"] > 0.02 for i in animals):
            return False, "an animal in it"
        return True, "no subject"
    return False, "unknown genre"


# --- select ------------------------------------------------------------------


def toml_str(value: str) -> str:
    # A JSON string is a valid TOML basic string for everything Commons titles
    # and author names contain.
    return json.dumps(value, ensure_ascii=False)


def write_sources(sources: list[dict]) -> None:
    lines = [
        "# The photographs of the public culling benchmark: Wikimedia Commons",
        "# quality images, each under the free licence named with it.",
        "# Written by `scripts/cull_bench.py select`; see testdata/cull_bench/README.md.",
        "# Only the measurements made from them are in this repository, not the",
        "# photographs.",
        "",
    ]
    for s in sources:
        lines += [
            "[[source]]",
            f"key = {toml_str(s['key'])}",
            f"genre = {toml_str(s['genre'])}",
            f"title = {toml_str(s['title'])}",
            f"page = {toml_str(s['page'])}",
            f"license = {toml_str(s['license'])}",
            f"license_url = {toml_str(s['license_url'])}",
            f"author = {toml_str(s['author'])}",
            f"width = {int(s['width'])}",
            f"height = {int(s['height'])}",
            f"why = {toml_str(s['why'])}",
            "",
        ]
    SOURCES_TOML.parent.mkdir(parents=True, exist_ok=True)
    SOURCES_TOML.write_text("\n".join(lines), encoding="utf-8")


def read_sources() -> list[dict]:
    if not SOURCES_TOML.exists():
        sys.exit(f"{SOURCES_TOML} does not exist; run `select` first")
    with SOURCES_TOML.open("rb") as f:
        return tomllib.load(f).get("source", [])


def cmd_select(args: argparse.Namespace) -> None:
    detectors = Detectors()
    taken: set[str] = set()
    chosen: list[dict] = []
    genres = [GENRE_BY_NAME[g] for g in args.genres] if args.genres else GENRES
    for genre in genres:
        titles: list[str] = []
        for category in genre.categories:
            titles += category_files(category, genre.depth)
        # A fixed shuffle: the same categories give the same choice next year,
        # give or take files added since.
        titles = sorted(
            set(titles), key=lambda t: hashlib.sha256(t.encode()).hexdigest()
        )
        titles = [t for t in titles if t.lower().endswith((".jpg", ".jpeg"))]
        print(f"{genre.name}: {len(titles)} candidate files", flush=True)
        accepted = 0
        for start in range(0, len(titles), 50):
            if accepted >= args.per_genre:
                break
            infos = image_infos(titles[start : start + 50])
            for title in titles[start : start + 50]:
                if accepted >= args.per_genre:
                    break
                if title in taken:
                    continue
                info = infos.get(title)
                if not info or info.get("mime") != "image/jpeg":
                    continue
                if max(info["width"], info["height"]) < LONG_EDGE:
                    continue
                meta = info.get("extmetadata", {})
                licence = plain_text(meta.get("LicenseShortName", {}).get("value", ""))
                if not LICENCE_RE.match(licence):
                    continue
                source = {
                    "key": source_key(title),
                    "genre": genre.name,
                    "title": title,
                    "page": info.get("descriptionurl")
                    or "https://commons.wikimedia.org/wiki/"
                    + urllib.parse.quote(title.replace(" ", "_")),
                    "license": licence,
                    "license_url": plain_text(
                        meta.get("LicenseUrl", {}).get("value", "")
                    ),
                    "author": plain_text(meta.get("Artist", {}).get("value", ""))
                    or "unknown",
                    "width": info["width"],
                    "height": info["height"],
                }
                try:
                    image = download_base(source)
                except Exception as e:  # noqa: BLE001 - one file, keep going
                    print(f"  skip {title}: {e}", flush=True)
                    continue
                analysis = load_analysis(source["key"])
                if analysis is None:
                    analysis = detectors.analyse(image)
                    save_analysis(source["key"], analysis)
                ok, why = accepts(analysis, genre.name)
                if not ok:
                    continue
                source["why"] = why
                taken.add(title)
                chosen.append(source)
                accepted += 1
                print(f"  [{accepted}/{args.per_genre}] {title}: {why}", flush=True)
        if accepted < args.per_genre:
            print(
                f"  only {accepted} of {args.per_genre} for {genre.name}", flush=True
            )
    if args.genres and SOURCES_TOML.exists():
        # Replacing some genres keeps the others.
        kept = [s for s in read_sources() if s["genre"] not in set(args.genres)]
        chosen = kept + chosen
    write_sources(chosen)
    print(f"wrote {len(chosen)} sources to {SOURCES_TOML}")


# --- make --------------------------------------------------------------------


def as_float(image: Image.Image) -> np.ndarray:
    return np.asarray(image, dtype=np.float32) / 255.0


def to_image(array: np.ndarray) -> Image.Image:
    return Image.fromarray(
        (np.clip(array, 0.0, 1.0) * 255.0 + 0.5).astype(np.uint8), "RGB"
    )


def gaussian(image: Image.Image, sigma: float) -> np.ndarray:
    return as_float(image.filter(ImageFilter.GaussianBlur(radius=sigma)))


def line_blur(array: np.ndarray, length: int, angle_degrees: float) -> np.ndarray:
    """Camera shake: a straight streak `length` pixels long at an angle, as the
    mean of the frame shifted to each whole-pixel point along the streak."""
    c = length // 2
    a = math.radians(angle_degrees)
    offsets = sorted(
        {
            (int(round(t * math.cos(a))), int(round(t * math.sin(a))))
            for t in np.linspace(-c, c, length * 4)
        }
    )
    h, w = array.shape[:2]
    padded = np.pad(array, ((c, c), (c, c), (0, 0)), mode="edge")
    out = np.zeros_like(array)
    for dx, dy in offsets:
        out += padded[c + dy : c + dy + h, c + dx : c + dx + w]
    return out / len(offsets)


def horizontal_blur(array: np.ndarray, length: int) -> np.ndarray:
    """A pan: every row smeared sideways, by a running mean."""
    half = length // 2
    padded = np.pad(array, ((0, 0), (half, half), (0, 0)), mode="edge")
    cumulative = np.cumsum(padded, axis=1, dtype=np.float64)
    cumulative = np.concatenate(
        [np.zeros_like(cumulative[:, :1]), cumulative], axis=1
    )
    width = array.shape[1]
    window = cumulative[:, length : length + width] - cumulative[:, :width]
    return (window / length).astype(np.float32)


def soft_mask(binary: np.ndarray, size: tuple[int, int], grow: float, feather: float) -> np.ndarray:
    """A mask at base size, grown by `grow` px and feathered over `feather` px."""
    mask = Image.fromarray((binary > 0).astype(np.uint8) * 255, "L").resize(
        size, Image.Resampling.BILINEAR
    )
    if grow > 0:
        # A blur and a low threshold grow the mask by about `grow` pixels in
        # linear time; PIL's MaxFilter does the same in time square in the
        # kernel, minutes per frame at this size.
        mask = mask.filter(ImageFilter.GaussianBlur(radius=grow / 2)).point(
            lambda v: 255 if v > 12 else 0
        )
    if feather > 0:
        mask = mask.filter(ImageFilter.GaussianBlur(radius=feather))
    return (np.asarray(mask, dtype=np.float32) / 255.0)[..., None]


def srgb_to_linear(x: np.ndarray) -> np.ndarray:
    return np.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(x: np.ndarray) -> np.ndarray:
    x = np.clip(x, 0.0, 1.0)
    return np.where(x <= 0.0031308, x * 12.92, 1.055 * x ** (1 / 2.4) - 0.055)


def seed_of(key: str) -> int:
    return int(hashlib.sha256(key.encode()).hexdigest()[:8], 16)


def make_variant(
    variant: Variant, image: Image.Image, analysis: dict, genre: str, key: str
) -> Image.Image | None:
    """The frame with its defect, or None when the photograph cannot carry it."""
    sharp = as_float(image)
    long_edge = max(image.size)
    rng = np.random.default_rng(seed_of(key + variant.name))
    subject = subject_instances(analysis, genre)
    union = None
    if subject:
        union = np.zeros_like(subject[0]["mask"], dtype=np.uint8)
        for inst in subject:
            union |= inst["mask"].astype(np.uint8)
    area = subject_area(analysis, genre)
    has_subject = (
        union is not None
        and 0.015 <= area <= 0.65
        and not ambiguous_subject(analysis, genre)
    )

    if variant.needs == "subject" and not has_subject:
        return None
    if variant.needs == "panning" and not (has_subject and genre in PANNING_GENRES):
        return None

    name = variant.name
    if name == "original":
        return to_image(sharp)
    if name == "blur_mild":
        return to_image(gaussian(image, 0.0005 * long_edge))
    if name == "blur_strong":
        return to_image(gaussian(image, 0.0022 * long_edge))
    if name == "motion":
        angle = float(rng.uniform(0, 180))
        return to_image(line_blur(sharp, int(0.006 * long_edge), angle))
    if name == "eyes_soft":
        people = [p for p in prominent_eyes(analysis) if p["iod"] >= 45]
        if union is not None:
            # Only the subject's eyes: a spectator's or a passer-by's soft eyes
            # are not a missed focus, and the backend rightly ignores them.
            if ambiguous_subject(analysis, genre):
                return None
            mh, mw = union.shape
            iw, ih = image.size

            def on_subject(person: dict) -> bool:
                x = sum(e[0] for e in person["eyes"]) / len(person["eyes"])
                y = sum(e[1] for e in person["eyes"]) / len(person["eyes"])
                mx = min(int(x * mw / iw), mw - 1)
                my = min(int(y * mh / ih), mh - 1)
                return bool(union[my, mx])

            people = [p for p in people if on_subject(p)]
        if not people:
            return None
        h, w = sharp.shape[:2]
        alpha = np.zeros((h, w), dtype=np.float32)
        yy, xx = np.mgrid[0:h, 0:w]
        sigma = 0.0
        for person in people:
            radius = 0.45 * person["iod"]
            sigma = max(sigma, 2.5, 0.06 * person["iod"])
            for ex, ey in person["eyes"]:
                d = np.sqrt((xx - ex) ** 2 + (yy - ey) ** 2)
                alpha = np.maximum(
                    alpha, np.clip((radius - d) / (0.35 * radius) + 0.5, 0, 1)
                )
        soft = gaussian(image, sigma)
        return to_image(sharp * (1 - alpha[..., None]) + soft * alpha[..., None])
    if name == "subject_soft":
        alpha = soft_mask(union, image.size, 0.004 * long_edge, 0.003 * long_edge)
        soft = gaussian(image, 0.0015 * long_edge)
        return to_image(sharp * (1 - alpha) + soft * alpha)
    if name == "background_soft":
        keep = soft_mask(union, image.size, 0.003 * long_edge, 0.002 * long_edge)
        soft = gaussian(image, 0.004 * long_edge)
        return to_image(sharp * keep + soft * (1 - keep))
    if name == "panning":
        keep = soft_mask(union, image.size, 0.003 * long_edge, 0.002 * long_edge)
        streaked = horizontal_blur(sharp, int(0.025 * long_edge))
        return to_image(sharp * keep + streaked * (1 - keep))
    if name in ("under", "over"):
        ev = -1.0 if name == "under" else 1.3
        return to_image(linear_to_srgb(srgb_to_linear(sharp) * 2.0**ev))
    if name == "noise":
        luminance = sharp.mean(axis=2, keepdims=True)
        grain = rng.normal(0.0, 1.0, sharp.shape[:2] + (1,)).astype(np.float32)
        chroma = rng.normal(0.0, 1.0, sharp.shape).astype(np.float32)
        noisy = sharp + (0.03 + 0.02 * (1 - luminance)) * grain + 0.012 * chroma
        return to_image(noisy)
    raise ValueError(name)


def cmd_make(args: argparse.Namespace) -> None:
    detectors = None
    sources = read_sources()
    manifest = []
    for index, source in enumerate(sources):
        key, genre = source["key"], source["genre"]
        image = download_base(source)
        analysis = load_analysis(key)
        if analysis is None:
            detectors = detectors or Detectors()
            analysis = detectors.analyse(image)
            save_analysis(key, analysis)
        out_dir = CACHE / "variants" / key
        out_dir.mkdir(parents=True, exist_ok=True)
        frames = []
        for variant in VARIANTS:
            path = out_dir / f"{variant.name}.jpg"
            if not path.exists() or (
                args.force and (not args.genres or genre in args.genres)
            ):
                frame = make_variant(variant, image, analysis, genre, key)
                if frame is None:
                    continue
                frame.save(path, quality=95, subsampling=0)
            frames.append({"variant": variant.name, "path": str(path)})
        manifest.append(
            {
                "key": key,
                "genre": genre,
                "index": index,
                "frames": frames,
            }
        )
        print(
            f"[{index + 1}/{len(sources)}] {genre} {key}: "
            + ", ".join(f["variant"] for f in frames),
            flush=True,
        )
    (CACHE / "manifest.json").write_text(json.dumps(manifest, indent=1))
    print(f"wrote {sum(len(m['frames']) for m in manifest)} frames")


# --- run ---------------------------------------------------------------------


def read_token() -> str:
    token = os.environ.get("LRG_API_TOKEN")
    if token:
        return token.strip()
    if sys.platform == "darwin":
        path = Path.home() / "Library/Application Support/LrGeniusAI/api-token"
    elif os.name == "nt":
        path = Path(os.environ["USERPROFILE"]) / "AppData/Local/LrGeniusAI/api-token"
    else:
        base = os.environ.get("XDG_CONFIG_HOME") or str(Path.home() / ".config")
        path = Path(base) / "LrGeniusAI" / "api-token"
    return path.read_text().strip()


def backend(url: str, token: str, route: str, body: dict, timeout: int = 1800) -> dict:
    request = urllib.request.Request(
        url.rstrip("/") + route,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Authorization": f"Bearer {token}"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.load(response)


# Series are an hour apart, so no two ever group; the frames of one series are
# 0.3 s apart, inside every preset's burst window.
EPOCH = 1_800_000_000.0
SERIES_GAP = 3600.0
FRAME_GAP = 0.3


def frame_order(key: str, names: list[str]) -> list[str]:
    """Shot order inside a series: fixed per series, never sorted by quality,
    so that no tie-break by position or id can pass for a judgement."""
    return sorted(names, key=lambda n: hashlib.sha256((key + n).encode()).hexdigest())


def cmd_run(args: argparse.Namespace) -> None:
    manifest = json.loads((CACHE / "manifest.json").read_text())
    token = read_token()
    variants = {v.name: v for v in VARIANTS}
    genres = args.genres or [g.name for g in GENRES]

    for genre_name in genres:
        genre = GENRE_BY_NAME[genre_name]
        series = [m for m in manifest if m["genre"] == genre_name]
        if not series:
            continue
        frames = []
        for s in series:
            by_name = {f["variant"]: f for f in s["frames"]}
            for position, name in enumerate(frame_order(s["key"], list(by_name))):
                frames.append(
                    {
                        "photo_id": f"bench-{s['key']}-{name}",
                        "path": by_name[name]["path"],
                        "date_time_unix": EPOCH
                        + s["index"] * SERIES_GAP
                        + position * FRAME_GAP,
                        "series": s["key"],
                        "variant": name,
                    }
                )

        if not args.skip_index:
            warnings: list[str] = []
            started = time.time()
            for i in range(0, len(frames), args.batch):
                batch = frames[i : i + args.batch]
                result = backend(
                    args.url,
                    token,
                    "/v1/index/photos/by-path",
                    {
                        "images": [
                            {
                                "path": f["path"],
                                "photo_id": f["photo_id"],
                                "date_time_unix": f["date_time_unix"],
                            }
                            for f in batch
                        ],
                        "tasks": "cull,embeddings",
                        "regenerate_metadata": "true" if args.reindex else "false",
                    },
                )
                if result.get("failure_count"):
                    print(f"  failures: {result.get('error_messages')}", flush=True)
                warnings += result.get("warnings") or []
                done = min(i + args.batch, len(frames))
                rate = (time.time() - started) / done
                print(
                    f"  {genre_name}: indexed {done}/{len(frames)} "
                    f"({rate:.1f} s/frame)",
                    flush=True,
                )
            for w in sorted(set(warnings))[:10]:
                print(f"  warning: {w}")

        graded = backend(
            args.url,
            token,
            "/v1/cull/grade",
            {
                "photo_ids": [f["photo_id"] for f in frames],
                "culling_preset": genre.preset,
                "include_stored_metadata": True,
                "include_embeddings": True,
            },
        )
        for w in graded.get("warnings") or []:
            print(f"  cull warning: {w}")
        produced = {
            p["photo_id"]: p for g in graded.get("groups", []) for p in g["photos"]
        }

        groups = []
        for s in series:
            photos = []
            for f in (f for f in frames if f["series"] == s["key"]):
                p = produced.get(f["photo_id"])
                if p is None:
                    print(f"  not graded: {f['photo_id']}")
                    continue
                v = variants[f["variant"]]
                stored = p.get("stored_metadata") or {}
                entry = {
                    "photo_id": f["photo_id"],
                    "filename": f"{f['photo_id']}.jpg",
                    "capture_time": stored.get("capture_time", f["date_time_unix"]),
                    "phash": stored.get("cull_phash"),
                    "metadata": stored,
                    "rank": v.rank,
                    "defect": v.name,
                }
                if p.get("embedding_q8"):
                    entry["embedding_q8"] = p["embedding_q8"]
                for flag in ("reject", "reject_optional", "must_keep"):
                    if getattr(v, flag):
                        entry[flag] = True
                if v.expect:
                    entry["expect_reasons"] = list(v.expect)
                if v.forbid:
                    entry["forbid_reasons"] = list(v.forbid)
                photos.append(entry)
            groups.append({"group_id": s["key"], "photos": photos})

        fixture = {
            "name": f"bench-{genre_name}",
            "notes": (
                "Public benchmark, generated by scripts/cull_bench.py from "
                "Wikimedia Commons quality images (see testdata/cull_bench/"
                "sources.toml). Each group is one photograph and copies of it "
                "with one defect each; labels are true by construction."
            ),
            "culling_preset": genre.preset,
            "groups_are_authoritative": True,
            "groups": groups,
        }
        FIXTURES_DIR.mkdir(parents=True, exist_ok=True)
        out = FIXTURES_DIR / f"{genre_name}.json"
        out.write_text(json.dumps(fixture, separators=(",", ":")) + "\n")
        print(f"wrote {out} ({len(groups)} series, {len(frames)} frames)")


def main() -> None:
    parser = argparse.ArgumentParser(description=(__doc__ or "").split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("select", help="pick photographs, write sources.toml")
    p.add_argument("--per-genre", type=int, default=24)
    p.add_argument("--genres", nargs="*", choices=list(GENRE_BY_NAME))
    p.set_defaults(func=cmd_select)

    p = sub.add_parser("make", help="download and make the series")
    p.add_argument("--force", action="store_true", help="make every frame again")
    p.add_argument(
        "--genres",
        nargs="*",
        choices=list(GENRE_BY_NAME),
        help="with --force, only these genres' frames",
    )
    p.set_defaults(func=cmd_make)

    p = sub.add_parser("run", help="index on a backend, write fixtures")
    p.add_argument("--url", default="http://127.0.0.1:19830")
    p.add_argument("--batch", type=int, default=8)
    p.add_argument("--genres", nargs="*", choices=list(GENRE_BY_NAME))
    p.add_argument(
        "--reindex",
        action="store_true",
        help="measure every frame again (after a change to what indexing stores)",
    )
    p.add_argument(
        "--skip-index",
        action="store_true",
        help="only grade, on what the backend already has",
    )
    p.set_defaults(func=cmd_run)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
