import argparse
import json
import os
import shutil
from pathlib import Path
from huggingface_hub import snapshot_download

def prepare_model_bundle(model_id: str, output_dir: str):
    """
    Prepares the CLIP model for bundling with the application.
    This script creates a bundled model directory with all necessary files
    including the config and weights.
    """
    hf_model_id = None
    config_file = Path("open_clip/model_configs") / f"{model_id}.json"
    
    if not config_file.exists():
        raise FileNotFoundError(f"Model config file not found for {model_id} at {config_file}")

    with open(config_file, "r") as f:
        model_cfg = json.load(f)
        if 'text_cfg' in model_cfg and 'hf_tokenizer_name' in model_cfg['text_cfg']:
            hf_model_id = model_cfg['text_cfg']['hf_tokenizer_name']
        elif model_cfg.get('hf_hub_id'):
            hf_model_id = model_cfg.get('hf_hub_id')

    if not hf_model_id:
        raise ValueError(f"Hugging Face Hub ID not found in model config for {model_id}")

    print(f"Using Hugging Face model ID: {hf_model_id}")

    # Download model from Hugging Face
    print(f"Downloading model {hf_model_id} from Hugging Face...")
    try:
        hf_cache_model_path = snapshot_download(repo_id=hf_model_id)
        print(f"Model downloaded to: {hf_cache_model_path}")
    except Exception as e:
        print(f"Failed to download model {hf_model_id}. Error: {e}")
        return

    # Create the complete config with preprocessing defaults
    vision_cfg = model_cfg.get('vision_cfg', {})
    image_size = vision_cfg.get('image_size')
    if isinstance(image_size, int):
        image_size = [image_size, image_size]

    preprocess_cfg = {
            "size": image_size or [384, 384],
            "mode": "RGB",
            "mean": model_cfg.get('mean', [0.5, 0.5, 0.5]),
            "std": model_cfg.get('std', [0.5, 0.5, 0.5]),
            "interpolation": model_cfg.get('interpolation', 'bicubic'),
            "resize_mode": "shortest",
            "fill_color": 0
    }

    complete_config = {
        "model_cfg": model_cfg,
        "preprocess_cfg": preprocess_cfg
    }

    # Create output directory
    os.makedirs(output_dir, exist_ok=True)

    # Write the config file
    config_output = os.path.join(output_dir, "open_clip_config.json")
    with open(config_output, "w") as f:
        json.dump(complete_config, f, indent=2)
    print(f"Created config: {config_output}")

    # Copy the model weights and tokenizer files
    source_dir = Path(hf_cache_model_path)
    if source_dir.exists():
        files_to_copy = ["open_clip_model.safetensors", "special_tokens_map.json", "tokenizer.json", "tokenizer_config.json", "config.json"]
        for file in files_to_copy:
            src = source_dir / file
            dst = Path(output_dir) / file
            if src.exists():
                shutil.copy2(src, dst)
                print(f"Copied: {file}")
            else:
                print(f"Warning: {file} not found at {src}")
    else:
        print(f"Warning: Source directory not found: {source_dir}")

    print(f"\nBundled model directory ready at: {output_dir}")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Prepare model bundle for LrGenius AI Server.")
    parser.add_argument("--model_id", type=str, default="ViT-SO400M-16-SigLIP2-384", help="The model ID from open_clip model configs.")
    parser.add_argument("--output_dir", type=str, default="dist/models", help="The output directory for the bundled model files.")
    args = parser.parse_args()

    prepare_model_bundle(args.model_id, args.output_dir)
