# train_clip.py
# Script to train an aesthetic score predictor using embeddings from a SentenceTransformer CLIP model.

import os
import numpy as np
import pandas as pd
from PIL import Image
from tqdm import tqdm
from sklearn.model_selection import train_test_split
from sentence_transformers import SentenceTransformer
import torch
import torch.nn as nn
import torch.optim as optim
from torch.utils.data import DataLoader, TensorDataset

# --- 1. CONFIGURATION ---

# Paths
OUTPUT_DIR = "./output"
EMBEDDINGS_FILE = os.path.join(OUTPUT_DIR, "clip_ava_embeddings.npz")
AESTHETIC_PTH_PATH = os.path.join(OUTPUT_DIR, "aesthetic_model_clip.pth")
MODEL_NAME = "clip-ViT-B-32-multilingual-v1"

# AVA Dataset paths (assuming they are available)
# IMPORTANT: Replace with the actual path to your AVA dataset files
AVA_LABELS_FILE = "/Volumes/EXTERN/AVA_dataset/AVA.txt"
AVA_IMAGES_DIR = "/Volumes/EXTERN/AVA_dataset/image"

# Training Parameters
EMBEDDING_BATCH_SIZE = 64
VALIDATION_SPLIT = 0.1
AESTHETIC_LEARNING_RATE = 0.001
AESTHETIC_EPOCHS = 50
AESTHETIC_BATCH_SIZE = 256
NUM_GENRES = 66 # As per AVA dataset documentation
EMBEDDING_DIM = 512 # CLIP ViT-B/32 uses 512-dimensional embeddings
EARLY_STOPPING_PATIENCE = 5 # Number of epochs to wait for improvement before stopping

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

# --- 4. CORE FUNCTIONS ---

def generate_embeddings():
    """ PHASE 1: Generate and save embeddings using the SentenceTransformer CLIP model. """
    if os.path.exists(EMBEDDINGS_FILE):
        print(f"Embeddings file already exists at {EMBEDDINGS_FILE}. Skipping.")
        return

    print("--- Starting Phase 1: Embedding Generation (SentenceTransformer) ---")

    print(f"Loading SentenceTransformer model: {MODEL_NAME}...")
    model = SentenceTransformer(MODEL_NAME)

    ava_dataset = AVADataset(AVA_LABELS_FILE, AVA_IMAGES_DIR)
    
    all_image_paths, all_scores_list, all_genres_list = [], [], []
    for path, score, genres in tqdm(ava_dataset, desc="Collecting valid dataset items"):
        all_image_paths.append(path)
        all_scores_list.append(score)
        all_genres_list.append(genres)

    print(f"Generating embeddings for {len(all_image_paths)} images...")
    
    all_embeds = []
    valid_scores = []
    valid_genres = []

    for i in tqdm(range(0, len(all_image_paths), EMBEDDING_BATCH_SIZE), desc="Generating embeddings in batches"):
        batch_paths = all_image_paths[i:i+EMBEDDING_BATCH_SIZE]
        batch_embeddings = model.encode(batch_paths, batch_size=len(batch_paths), convert_to_numpy=True, show_progress_bar=False)

        for j, emb in enumerate(batch_embeddings):
            if emb is not None:
                all_embeds.append(emb)
                original_index = i + j
                valid_scores.append(all_scores_list[original_index])
                valid_genres.append(all_genres_list[original_index])

    all_embeds = np.array(all_embeds)
    all_scores = np.array(valid_scores)
    all_genres = np.array(valid_genres)

    print(f"Saving {len(all_embeds)} items to {EMBEDDINGS_FILE}...")
    np.savez_compressed(
        EMBEDDINGS_FILE,
        embeddings=all_embeds,
        scores=all_scores,
        genres=all_genres,
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
    device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
    print(f"Training on device: {device}")
    model = AestheticPredictor(input_dim=EMBEDDING_DIM).to(device)
    criterion = nn.MSELoss()
    optimizer = optim.Adam(model.parameters(), lr=AESTHETIC_LEARNING_RATE)

    best_val_loss = float('inf')
    epochs_no_improve = 0

    print("\nTraining Aesthetic Probe with PyTorch...")
    for epoch in range(AESTHETIC_EPOCHS):
        model.train()
        train_loss = 0.0
        for inputs, targets in train_loader:
            inputs, targets = inputs.to(device), targets.to(device)
            
            optimizer.zero_grad()
            outputs = model(inputs)
            loss = criterion(outputs, targets)
            loss.backward()
            optimizer.step()
            train_loss += loss.item() * inputs.size(0)

        model.eval()
        val_loss = 0.0
        with torch.no_grad():
            for inputs, targets in val_loader:
                inputs, targets = inputs.to(device), targets.to(device)
                outputs = model(inputs)
                loss = criterion(outputs, targets)
                val_loss += loss.item() * inputs.size(0)

        train_loss /= len(train_loader.dataset)
        val_loss /= len(val_loader.dataset)

        print(f"Epoch {epoch+1}/{AESTHETIC_EPOCHS}, Train Loss: {train_loss:.4f}, Val Loss: {val_loss:.4f}")

        if val_loss < best_val_loss:
            best_val_loss = val_loss
            torch.save(model.state_dict(), AESTHETIC_PTH_PATH)
            print(f"Validation loss improved. Model saved to {AESTHETIC_PTH_PATH}")
            epochs_no_improve = 0
        else:
            epochs_no_improve += 1
            if epochs_no_improve >= EARLY_STOPPING_PATIENCE:
                print(f"Early stopping triggered after {EARLY_STOPPING_PATIENCE} epochs with no improvement.")
                break

    print("--- Phase 2 Finished ---")


if __name__ == "__main__":
    os.makedirs(OUTPUT_DIR, exist_ok=True)
    
    print("Please ensure you have downloaded the AVA dataset and updated the paths in this script.")
    print("You can find more info on the AVA dataset here: https://github.com/cs-chan/AVA-dataset")
    
    try:
        generate_embeddings()
        run_training_pipeline()
        
        print(f"\n\n✅ Training pipeline completed successfully!")
        print(f"The trained PyTorch model is available at: {AESTHETIC_PTH_PATH}")

    except FileNotFoundError as e:
        print(f"\n❌ ERROR: {e}")
        print("Please update the paths at the top of the script and try again.")
    except Exception as e:
        print(f"\n❌ An unexpected error occurred: {e}")