import os
from transformers import AutoProcessor, CLIPModel
from optimum.exporters.onnx import main_export
import shutil

# --- Konfiguration ---
# Der lokale Ordner mit den heruntergeladenen PyTorch-Dateien.
pytorch_source_dir = "./jina_clip_v2_pytorch_source"

# Der Zielordner für die finalen ONNX-Modelle.
onnx_output_dir = "./jina_clip_v2_onnx"

# Ein temporärer Ordner, um das Vision-Modell für einen sauberen Export zu isolieren.
temp_vision_export_dir = "./temp_vision_only_for_export"


# --- Hauptskript ---
def convert_local_pytorch_to_onnx():
    """
    Isoliert zuerst die Vision-Komponente eines lokalen CLIP-Modells und
    exportiert dann diese saubere Komponente nach ONNX.
    """
    print(f"Starte die Konvertierung von PyTorch nach ONNX.")
    print(f"Lese Quelldateien aus: '{pytorch_source_dir}'")

    if not os.path.exists(pytorch_source_dir):
        print(f"Fehler: Das Quellverzeichnis '{pytorch_source_dir}' wurde nicht gefunden.")
        print("Bitte führe zuerst das 'download_jina_model.py'-Skript aus.")
        return

    try:
        # --- SCHRITT 1: Isoliere das Vision-Modell ---
        print("\nLade das vollständige PyTorch CLIP-Modell, um die Vision-Komponente zu extrahieren...")
        full_model = CLIPModel.from_pretrained(pytorch_source_dir, trust_remote_code=True)
        vision_model = full_model.vision_model

        print(f"Speichere das isolierte Vision-Modell temporär in '{temp_vision_export_dir}'...")
        if os.path.exists(temp_vision_export_dir):
            shutil.rmtree(temp_vision_export_dir)
        os.makedirs(temp_vision_export_dir)
        vision_model.save_pretrained(temp_vision_export_dir)
        print("Vision-Modell erfolgreich isoliert.")

        # --- SCHRITT 2: Exportiere das isolierte Vision-Modell ---
        print("\nStarte den ONNX-Export des isolierten Vision-Modells...")
        # Stelle sicher, dass das finale Zielverzeichnis existiert.
        os.makedirs(onnx_output_dir, exist_ok=True)
        
        # Der Output ist jetzt ein direkter Dateipfad.
        output_onnx_file = os.path.join(onnx_output_dir, "vision_model.onnx")

        main_export(
            model_name_or_path=temp_vision_export_dir,  # Exportiere aus dem sauberen, temporären Ordner
            output=output_onnx_file,                    # Gib den exakten Zieldateinamen an
            task="feature-extraction",                  # Dieser Task ist jetzt eindeutig
            opset=14,
        )
        print("Vision-Modell erfolgreich nach ONNX exportiert.")

        # --- SCHRITT 3: Aufräumen und Prozessor kopieren ---
        print(f"\nBereinige das temporäre Verzeichnis '{temp_vision_export_dir}'...")
        shutil.rmtree(temp_vision_export_dir)

        print("\nKopiere Prozessor-Dateien...")
        processor = AutoProcessor.from_pretrained(pytorch_source_dir, trust_remote_code=True)
        processor.save_pretrained(onnx_output_dir)

        print("\n" + "="*50)
        print("✅ ONNX-Konvertierung erfolgreich abgeschlossen!")
        print(f"Die fertigen Modelle befinden sich jetzt in: {os.path.abspath(onnx_output_dir)}")
        print("\nNÄCHSTER SCHRITT:")
        print("Verwende die Datei 'vision_model.onnx' in deinem Inferenz-Skript ('train.py').")
        print("="*50)

    except Exception as e:
        print("\n" + "!"*50)
        print(f"Ein Fehler ist während der Konvertierung aufgetreten: {e}")
        print("!"*50)

if __name__ == "__main__":
    convert_local_pytorch_to_onnx()

