# Before running, make sure you have run convert_clip_to_onnx.py to generate the models.
# You also need to install the required packages:
# pip install onnxruntime numpy Pillow huggingface_hub tokenizers

import onnxruntime as ort
import numpy as np
from PIL import Image
from pathlib import Path
from huggingface_hub import hf_hub_download
from tokenizers import Tokenizer

def preprocess_image(image_path, size=224):
    """
    Preprocesses an image to be compatible with the CLIP vision model.
    This function replicates the preprocessing from open_clip without torch.
    """
    image = Image.open(image_path).convert("RGB")

    # Resize and center crop
    width, height = image.size
    short_dim = min(width, height)
    scale = size / short_dim
    new_width, new_height = int(width * scale), int(height * scale)
    image = image.resize((new_width, new_height), Image.BICUBIC)
    
    left = (new_width - size) / 2
    top = (new_height - size) / 2
    right = (new_width + size) / 2
    bottom = (new_height + size) / 2
    image = image.crop((left, top, right, bottom))

    # Convert to numpy array and normalize
    image_np = np.array(image).astype(np.float32) / 255.0
    mean = np.array([0.48145466, 0.4578275, 0.40821073], dtype=np.float32)
    std = np.array([0.26862954, 0.26130258, 0.27577711], dtype=np.float32)
    image_np = (image_np - mean) / std

    # Transpose from (H, W, C) to (C, H, W) and add batch dimension
    image_np = image_np.transpose(2, 0, 1)
    return np.expand_dims(image_np, axis=0)

# --------------------------
# 1. Configuration
# --------------------------
MODEL_DIR = Path("clip_vitl14_onnx")
VISION_MODEL_PATH = MODEL_DIR / "vision_model.onnx"
TEXT_MODEL_PATH = MODEL_DIR / "text_model.onnx"
IMAGE_PATH = Path("../../tests/images/Basti.jpg")
TOKENIZER_REPO_ID = "openai/clip-vit-large-patch14"


# --------------------------
# 2. Load models and tokenizer
# --------------------------
print("Loading ONNX models...")
vision_session = ort.InferenceSession(str(VISION_MODEL_PATH), providers=ort.get_available_providers())
text_session = ort.InferenceSession(str(TEXT_MODEL_PATH), providers=ort.get_available_providers())
print("Models loaded.")

print("Downloading and loading tokenizer...")
tokenizer_path = hf_hub_download(repo_id=TOKENIZER_REPO_ID, filename="tokenizer.json")
tokenizer = Tokenizer.from_file(tokenizer_path)
print("Tokenizer loaded.")


# --------------------------
# 3. Prepare inputs
# --------------------------
# Image preprocessing
print("Preprocessing image...")
preprocessed_image = preprocess_image(IMAGE_PATH)

# Text tokenization
print("Tokenizing text...")
texts = ["a photo of a man", "a photo of a dog", "a photo of a cathedral"]
# The tokenizer needs to be configured to pad the sequences to the model's expected input length (77 for CLIP)
tokenizer.enable_padding(pad_id=0, pad_token="<|endoftext|>", length=77)
tokenized_output = tokenizer.encode_batch(texts)
input_ids = np.array([encoding.ids for encoding in tokenized_output], dtype=np.int64)


# --------------------------
# 4. Run inference
# --------------------------
# Image embedding
print("Generating image embedding...")
vision_inputs = {"pixel_values": preprocessed_image}
image_features = vision_session.run(["image_features"], vision_inputs)[0]
image_embedding = image_features / np.linalg.norm(image_features, axis=-1, keepdims=True)


# Text embedding
print("Generating text embeddings...")
text_inputs = {"input_ids": input_ids}
text_features = text_session.run(["pooler_output"], text_inputs)[0]
text_embeddings = text_features / np.linalg.norm(text_features, axis=-1, keepdims=True)

# --------------------------
# 5. Compare embeddings
# --------------------------
print("Comparing embeddings...")
similarity_scores = (image_embedding @ text_embeddings.T).squeeze(0)

# --------------------------
# 6. Print results
# --------------------------
print("\nResults:")
for text, score in zip(texts, similarity_scores):
    print(f"Similarity between image and text \"{text}\": {score:.4f}")

# Expected output should show the highest similarity for "a photo of a man"
