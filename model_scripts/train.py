# train.py
# Script to train an aesthetic score predictor using embeddings from the UForm model.

import os
import numpy as np
import pandas as pd
from PIL import Image
from tqdm import tqdm
from sklearn.model_selection import train_test_split
from transformers import AutoProcessor, AutoModel
import torch
import torch.nn as nn
import torch.optim as optim
from torch.utils.data import DataLoader, TensorDataset, Dataset


# --- 1. CONFIGURATION ---

# Paths
OUTPUT_DIR = "./output"
EMBEDDINGS_FILE = os.path.join(OUTPUT_DIR, "ava_embeddings.npz")
AESTHETIC_MODEL_PATH = os.path.join(OUTPUT_DIR, "aesthetic_model.pth")
MODEL_PATH = "siglip-base-patch16-256-multilingual"

# AVA Dataset paths (assuming they are available)
# IMPORTANT: Replace with the actual path to your AVA dataset files
AVA_LABELS_FILE = "d:/AVA_dataset/AVA.txt"
AVA_IMAGES_DIR = "d:/AVA_dataset/image"

# Training Parameters
EMBEDDING_BATCH_SIZE = 256 # Adjusted for single-threaded loading.
EMBEDDING_WORKERS = 0 # Set to 0 to disable multiprocessing and avoid its overhead.
VALIDATION_SPLIT = 0.1
AESTHETIC_LEARNING_RATE = 0.001
AESTHETIC_EPOCHS = 50
AESTHETIC_BATCH_SIZE = 256
NUM_GENRES = 66 # As per AVA dataset documentation
EMBEDDING_DIM = 768 # For siglip-so400m-patch14-384

# --- 2. DATA HANDLING ---

class AVADataset:
    """ Dataset for loading AVA images, scores, and genre labels. """
    def __init__(self, labels_file, images_dir):
        self.images_dir = images_dir
        
        if not os.path.exists(labels_file):
            raise FileNotFoundError(f"AVA labels file not found at: {labels_file}. Please update the AVA_LABELS_FILE path in the script.")
        if not os.path.exists(images_dir):
            raise FileNotFoundError(f"AVA images directory not found at: {images_dir}. Please update the AVA_IMAGES_DIR path in the script.")

        print("Loading AVA labels...")
        ava_df = pd.read_csv(labels_file, sep=' ', header=None)
        
        self.image_ids = ava_df.iloc[:, 1].values
        
        score_columns = ava_df.iloc[:, 2:12].values
        # Handle division by zero for images with no votes
        sum_scores = np.sum(score_columns, axis=1)
        self.mean_scores = np.divide(np.tensordot(score_columns, np.arange(1, 11), axes=1), sum_scores, out=np.zeros_like(sum_scores, dtype=float), where=sum_scores!=0)

        tag_col1 = ava_df.iloc[:, 12].values
        tag_col2 = ava_df.iloc[:, 13].values
        self.genres = np.zeros((len(self.image_ids), NUM_GENRES), dtype=np.float32)
        for i, (tag1, tag2) in enumerate(zip(tag_col1, tag_col2)):
            if tag1 > 0 and tag1 <= NUM_GENRES: self.genres[i, tag1 - 1] = 1.0
            if tag2 > 0 and tag2 <= NUM_GENRES: self.genres[i, tag2 - 1] = 1.0

    def __iter__(self):
        for i in range(len(self.image_ids)):
            image_id = self.image_ids[i]
            image_path = os.path.join(self.images_dir, f"{image_id}.jpg")
            
            if not os.path.exists(image_path):
                continue
                
            score = self.mean_scores[i]
            genres = self.genres[i]
            yield image_path, score, genres
            
class AVAPathsDataset(Dataset):
    """ A PyTorch Dataset that returns image paths and labels for efficient loading. """
    def __init__(self, labels_file, images_dir):
        self.images_dir = images_dir
        print("Loading AVA labels for path dataset...")
        ava_df = pd.read_csv(labels_file, sep=' ', header=None)
        
        self.image_ids = ava_df.iloc[:, 1].values
        score_columns = ava_df.iloc[:, 2:12].values
        sum_scores = np.sum(score_columns, axis=1)
        self.mean_scores = np.divide(np.tensordot(score_columns, np.arange(1, 11), axes=1), sum_scores, out=np.zeros_like(sum_scores, dtype=float), where=sum_scores!=0)

    def __len__(self):
        return len(self.image_ids)

    def __getitem__(self, idx):
        image_id = self.image_ids[idx]
        image_path = os.path.join(self.images_dir, f"{image_id}.jpg")
        score = self.mean_scores[idx]
        return image_path, score

# --- 3. PYTORCH MODEL DEFINITION ---

class AestheticPredictor(nn.Module):
    """ A simple MLP to predict aesthetic score from an embedding. """
    def __init__(self, input_dim, hidden_dim1=256, hidden_dim2=128):
        super().__init__()
        self.layers = nn.Sequential(
            nn.Linear(input_dim, hidden_dim1),
            nn.ReLU(),
            nn.Linear(hidden_dim1, hidden_dim2),
            nn.ReLU(),
            nn.Linear(hidden_dim2, 1)
        )

    def forward(self, x):
        return self.layers(x)

def generate_embeddings():
    """ PHASE 1: Generate and save embeddings. """
    if os.path.exists(EMBEDDINGS_FILE):
        print(f"Embeddings file already exists at {EMBEDDINGS_FILE}. Skipping.")
        return

    print("--- Starting Phase 1: Embedding Generation ---")

    model_image = AutoModel.from_pretrained(MODEL_PATH)
    processor_image = AutoProcessor.from_pretrained(MODEL_PATH)

    if torch.cuda.is_available():
        model_image.to("cuda")
        device = "cuda"
        print("Using CUDA")
    elif torch.backends.mps.is_available():
        model_image.to("mps")
        device = "mps"
        print("Using MPS")
    else:
        device = "cpu"
        print("Using CPU")
    
    # Revert to a single-threaded loop which is faster for simple I/O on Windows
    # by avoiding multiprocessing overhead.
    ava_dataset = AVADataset(AVA_LABELS_FILE, AVA_IMAGES_DIR)
    
    all_embeds = []
    all_scores = []
    
    batch_data = []
    total_items = len(ava_dataset.image_ids)

    print("Generating embeddings (single-threaded)...")
    for path, score, _ in tqdm(ava_dataset, total=total_items, desc="Collecting images"):
        if not os.path.exists(path):
            continue
        batch_data.append((path, score))

        if len(batch_data) >= EMBEDDING_BATCH_SIZE:
            paths, scores = zip(*batch_data)
            try:
                images = [Image.open(p).convert("RGB") for p in paths]
                with torch.no_grad():
                    inputs = processor_image(images=images, return_tensors="pt").to(device)
                    embeddings = model_image.get_image_features(**inputs)
                all_embeds.extend(embeddings.cpu().numpy())
                all_scores.extend(scores)
            except Exception as e:
                print(f"Error processing a batch, skipping. Error: {e}")
            batch_data = []

    # Process the final partial batch
    if batch_data:
        paths, scores = zip(*batch_data)
        images = [Image.open(p).convert("RGB") for p in paths]
        with torch.no_grad():
            inputs = processor_image(images=images, return_tensors="pt").to(device)
            embeddings = model_image.get_image_features(**inputs)
        all_embeds.extend(embeddings.cpu().numpy())
        all_scores.extend(scores)
    
    print(f"Saving {len(all_embeds)} items to {EMBEDDINGS_FILE}...")
    np.savez_compressed(
        EMBEDDINGS_FILE,
        embeddings=np.array(all_embeds),
        scores=np.array(all_scores),
        embedding_dim=EMBEDDING_DIM)
    print("--- Phase 1 Finished ---")


def run_training_pipeline():
    """ PHASE 2: Train aesthetic probe model using PyTorch. """
    print("\n--- Starting Phase 2: Training Aesthetic Probe (PyTorch) ---")
    if not os.path.exists(EMBEDDINGS_FILE):
        print(f"Embeddings file not found at {EMBEDDINGS_FILE}. Please run embedding generation first.")
        return
        
    data = np.load(EMBEDDINGS_FILE)
    X = data['embeddings']
    y = data['scores']

    X_train, X_val, y_train, y_val = train_test_split(X, y, test_size=VALIDATION_SPLIT, random_state=42)

    # Create PyTorch datasets and dataloaders
    train_dataset = TensorDataset(torch.from_numpy(X_train).float(), torch.from_numpy(y_train).float().view(-1, 1))
    val_dataset = TensorDataset(torch.from_numpy(X_val).float(), torch.from_numpy(y_val).float().view(-1, 1))
    train_loader = DataLoader(train_dataset, batch_size=AESTHETIC_BATCH_SIZE, shuffle=True)
    val_loader = DataLoader(val_dataset, batch_size=AESTHETIC_BATCH_SIZE)

    # Initialize model, loss, and optimizer
    device = torch.device("cuda" if torch.cuda.is_available() else "mps" if torch.backends.mps.is_available() else "cpu")
    print(f"Training on device: {device}")
    model = AestheticPredictor(input_dim=EMBEDDING_DIM).to(device)
    criterion = nn.MSELoss()
    optimizer = optim.Adam(model.parameters(), lr=AESTHETIC_LEARNING_RATE)

    print("\nTraining Aesthetic Probe with PyTorch...")
    for epoch in range(AESTHETIC_EPOCHS):
        model.train()
        for inputs, targets in train_loader:
            inputs, targets = inputs.to(device), targets.to(device)
            optimizer.zero_grad()
            outputs = model(inputs)
            loss = criterion(outputs, targets)
            loss.backward()
            optimizer.step()

        # Simple validation loss for progress
        model.eval()
        val_loss = 0.0
        with torch.no_grad():
            for inputs, targets in val_loader:
                inputs, targets = inputs.to(device), targets.to(device)
                outputs = model(inputs)
                loss = criterion(outputs, targets)
                val_loss += loss.item()
        
        print(f"Epoch {epoch+1}/{AESTHETIC_EPOCHS}, Validation Loss: {val_loss/len(val_loader):.4f}")

    torch.save(model.state_dict(), AESTHETIC_MODEL_PATH)
    print(f"PyTorch model saved to {AESTHETIC_MODEL_PATH}")
    
    print("--- Phase 2 Finished ---")

if __name__ == "__main__":
    # --- Create output directory ---
    os.makedirs(OUTPUT_DIR, exist_ok=True)
    
    # --- Run the full pipeline ---
    print("Please ensure you have downloaded the AVA dataset and updated the paths in this script.")
    print("You can find more info on the AVA dataset here: https://github.com/cs-chan/AVA-dataset")
    
    try:
        generate_embeddings()
        run_training_pipeline()
        
        print("\n\n✅ Training pipeline completed successfully!")
        print(f"The trained PyTorch model is available at: {AESTHETIC_MODEL_PATH}")

    except FileNotFoundError as e:
        print(f"\n❌ ERROR: {e}")
        print("Please update the paths at the top of the script and try again.")
    except Exception as e:
        print(f"\n❌ An unexpected error occurred: {e}")