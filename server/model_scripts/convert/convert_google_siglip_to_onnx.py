import os
from pathlib import Path
from PIL import Image
import requests
import torch
import numpy as np
import onnxruntime as ort
from transformers import SiglipModel, AutoProcessor

def convert_siglip_to_onnx(model_id: str, output_path: Path):
    """
    Exports both the vision and text encoders of a SigLIP model to separate ONNX files.
    Includes a workaround for a bug in SiglipProcessor that fails to create an attention_mask.
    """
    print(f"Starting conversion for model: {model_id}")
    output_path.mkdir(parents=True, exist_ok=True)

    print("Loading base PyTorch model...")
    model = SiglipModel.from_pretrained(model_id)
    processor = AutoProcessor.from_pretrained(model_id)
    
    vision_model_path = output_path / "vision_model.onnx"
    text_model_path = output_path / "text_model.onnx"

    # --- Export the Vision Encoder (no changes needed here) ---
    print(f"Exporting vision model to {vision_model_path}...")
    vision_inputs = processor(images=Image.new("RGB", (224, 224)), return_tensors="pt")
    torch.onnx.export(
        model.vision_model,
        (vision_inputs['pixel_values']),
        vision_model_path,
        input_names=['pixel_values'],
        output_names=['image_embeds'],
        dynamic_axes={'pixel_values': {0: 'batch_size'}},
        opset_version=14
    )

    # --- Export the Text Encoder with WORKAROUND ---
    print(f"Exporting text model to {text_model_path}...")
    
    # 1. Get the input_ids from the processor.
    text_inputs = processor(text=["a sample"], padding=True, return_tensors="pt")
    input_ids = text_inputs['input_ids']
    
    # 2. WORKAROUND: Manually create the attention_mask because the processor fails to.
    attention_mask = torch.ones_like(input_ids)
    
    # 3. Export the model using the manually created mask.
    torch.onnx.export(
        model.text_model,
        (input_ids, attention_mask), # Pass the two tensors directly
        text_model_path,
        input_names=['input_ids', 'attention_mask'],
        output_names=['last_hidden_state', 'pooler_output'],
        dynamic_axes={
            'input_ids': {0: 'batch_size', 1: 'sequence_length'},
            'attention_mask': {0: 'batch_size', 1: 'sequence_length'}
        },
        opset_version=14
    )

    processor.save_pretrained(output_path)
    
    print("-" * 50)
    print("✅ Conversion successful!")
    print(f"   - Vision model saved to: {vision_model_path}")
    print(f"   - Text model saved to: {text_model_path}")
    print(f"   - Processor saved to: {output_path}")
    print("-" * 50)

def verify_onnx_model(model_path: Path):
    """
    Loads and verifies the separate ONNX vision and text models.
    """
    print("\nVerifying the converted ONNX models...")
    vision_model_path = model_path / "vision_model.onnx"
    text_model_path = model_path / "text_model.onnx"

    if not vision_model_path.exists() or not text_model_path.exists():
        print("❌ Cannot verify: One or both ONNX model files are missing.")
        return

    try:
        processor = AutoProcessor.from_pretrained(model_path)
        vision_session = ort.InferenceSession(str(vision_model_path))
        text_session = ort.InferenceSession(str(text_model_path))

        url = "http://images.cocodataset.org/val2017/000000039769.jpg"
        image = Image.open(requests.get(url, stream=True).raw)
        texts = ["a photo of 2 cats", "a photo of 2 dogs"]
        
        # We use return_tensors="np" for onnxruntime
        inputs = processor(text=texts, images=image, padding="max_length", return_tensors="np")

        # Run inference on the vision model
        image_embeds = vision_session.run(None, {'pixel_values': inputs['pixel_values']})[0]

        # Run inference on the text model
        # The same bug might not occur when processing image and text together,
        # but we use the same robust method to be safe.
        input_ids = inputs['input_ids']
        attention_mask = inputs.get('attention_mask', np.ones_like(input_ids))

        model_outputs = text_session.run(None, {
            'input_ids': input_ids,
            'attention_mask': attention_mask
        })

        last_hidden_state = model_outputs[0]
        pooler_output = model_outputs[1]

        print("✅ Verification successful!")
        print(f"   Image embeddings shape: {image_embeds.shape}")
        print(f"   Last hidden state shape: {last_hidden_state.shape}")
        print(f"   Pooler output shape: {pooler_output.shape}")
        print("   Both ONNX models are ready for use.")

    except Exception as e:
        print(f"❌ An error occurred during verification: {e}")

if __name__ == "__main__":
    MODEL_ID = "google/siglip-base-patch16-224"
    OUTPUT_DIR = Path("siglip-base-onnx-separate")

    convert_siglip_to_onnx(model_id=MODEL_ID, output_path=OUTPUT_DIR)
    verify_onnx_model(model_path=OUTPUT_DIR)