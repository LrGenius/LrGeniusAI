# config.py
# All configurable settings for the training script.

# --- PATHS ---
# Path to the root of the AVA dataset directory
# This directory should contain AVA.txt and the 'images' folder
AVA_ROOT_DIR = "/Volumes/EXTERN/AVA_dataset"

# Path to the AVA.txt file
AVA_LABELS_FILE = f"{AVA_ROOT_DIR}/AVA.txt"

# Path to the folder containing all AVA images
AVA_IMAGES_DIR = f"{AVA_ROOT_DIR}/image"

# Directory to save trained models and generated files
OUTPUT_DIR = "./models"

# Path to save/load the generated embeddings
EMBEDDINGS_FILE = f"{OUTPUT_DIR}/ava_onnx_clip_embeddings_with_genres.npz"

# Path to save the final trained aesthetic probe model (PyTorch format)
AESTHETIC_MODEL_PATH = f"{OUTPUT_DIR}/aesthetic_probe.pth"

# Path to save the final trained genre probe model (PyTorch format)
GENRE_MODEL_PATH = f"{OUTPUT_DIR}/genre_probe.pth"

# Path to save the exported aesthetic model (ONNX format)
AESTHETIC_ONNX_PATH = f"{OUTPUT_DIR}/aesthetic_probe.onnx"

# Path to save the exported genre model (ONNX format)
GENRE_ONNX_PATH = f"{OUTPUT_DIR}/genre_probe.onnx"


# --- MODEL CONFIG ---
# Sentence Transformer model to use for generating embeddings
BASE_MODEL_NAME = "../../models/clip"
PRETRAINED = "" # Not used with Sentence Transformers

# The dimension of the embeddings produced by the base model
EMBEDDING_DIM = 512


# --- AESTHETIC TRAINING CONFIG ---
# Batch size for generating embeddings (adjust based on your VRAM)
# A 12GB card like the 4070 Ti should handle 128 or 256 easily.
EMBEDDING_BATCH_SIZE = 64

# Batch size for training the aesthetic probe
AESTHETIC_BATCH_SIZE = 512

# Number of epochs to train the aesthetic probe
AESTHETIC_EPOCHS = 10

# Learning rate for the aesthetic probe optimizer
AESTHETIC_LEARNING_RATE = 1e-3


# --- GENRE TRAINING CONFIG ---
# Batch size for training the genre probe
GENRE_BATCH_SIZE = 512

# Number of epochs to train the genre probe
GENRE_EPOCHS = 10

# Learning rate for the genre probe optimizer
GENRE_LEARNING_RATE = 1e-3


# --- GENERAL CONFIG ---
# Percentage of the dataset to use for validation
VALIDATION_SPLIT = 0.1

# List of the 66 genre/style labels from the AVA dataset.
# The index in this list corresponds to the tag_id - 1.
GENRE_LABELS = [
    "Abstract", "Action", "Animal", "Architecture", "Astro", "Black and White",
    "Blueprint", "Blured background", "Botanical", "Candid", "Cityscape",
    "Still life (cluttered)", "Complementary Colors", "Conceptual", "DOF",
    "Drama", "Fashion", "Fine art", "Food", "Funny", "Genre scene", "HDR",
    "High contrast", "Horizontal line", "Interior", "Landscape", "Leading Lines",
    "Long exposure", "Macro", "Minimalism", "Monochrome", "Motion", "Nature",
    "Negative space", "Neon", "Newborn", "Night", "Nude", "Old", "Pastel",
    "Pattern", "People", "Perspective", "Pets", "Portrait", "Repetition",
    "Romantic", "Rural", "Sea", "Selective color", "Silhouette", "Sky",
    "Social", "Sport", "Still life", "Street", "Symmetry", "Texture",
    "Travel", "Urban", "Vanitas", "Vertical line", "Vintage", "Water",
    "Wedding", "White background"
]
NUM_GENRES = len(GENRE_LABELS)