"""M4 golden fixtures: real SigLIP2 image+text embeddings from the torch
model, for deterministic synthetic images and canned queries, so the
Rust ort pipeline (preprocessing + inference) can be verified end-to-end
(not just the exported ONNX graph in isolation, which spike S1 already
checked)."""
import json, os, sys
import numpy as np
import torch
import open_clip
from huggingface_hub import hf_hub_download
from PIL import Image

MODEL_NAME = "ViT-SO400M-16-SigLIP2-384"
weights = hf_hub_download("timm/" + MODEL_NAME, "open_clip_model.safetensors", local_files_only=True)
model, _, preprocess = open_clip.create_model_and_transforms(MODEL_NAME, pretrained=weights)
tokenizer = open_clip.get_tokenizer(MODEL_NAME)
model.eval()


def lcg_bytes(n, seed=42):
    out = np.empty(n, dtype=np.uint8)
    x = seed
    for i in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        out[i] = (x >> 16) & 0xFF
    return out


def gradient(w, h):
    n = w * h * 3
    vals = (np.arange(n, dtype=np.float64) * 255.0 / (n - 1)).astype(np.uint8)
    return vals.reshape(h, w, 3)


images = {
    "gradient_640x480": gradient(640, 480),
    "lcg_noise_800x600": lcg_bytes(800 * 600 * 3).reshape(600, 800, 3),
    "small_320x240": lcg_bytes(320 * 240 * 3, seed=99).reshape(240, 320, 3),
}

image_embeddings = {}
for name, arr in images.items():
    pil = Image.fromarray(arr, "RGB")
    tensor = preprocess(pil).unsqueeze(0)
    with torch.no_grad():
        emb = model.encode_image(tensor)[0].numpy()
    image_embeddings[name] = {
        "width": int(arr.shape[1]),
        "height": int(arr.shape[0]),
        "embedding": emb.tolist(),
    }

texts = [
    "a photo of a dog on a beach at sunset",
    "portrait of a woman with red hair",
    "ein Foto von einem Hund am Strand",
    "夕日のビーチにいる犬の写真",
    "Photo_of a CAT!!!  with   spaces",
]
token_ids = tokenizer(texts)
with torch.no_grad():
    text_emb = model.encode_text(token_ids).numpy()
text_embeddings = {t: text_emb[i].tolist() for i, t in enumerate(texts)}

dst = "/Users/bm/src/LrGeniusAI/server-rs/testdata/siglip_goldens.json"
json.dump({"images": image_embeddings, "texts": text_embeddings}, open(dst, "w"))
print("wrote", dst, "images:", list(images.keys()))
