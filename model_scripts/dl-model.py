import os
from huggingface_hub import snapshot_download


# --- Konfiguration ---
# Die ID des Jina CLIP v2 Modells - neuer, mehrsprachig und Apache 2.0 Lizenz.
model_id = "google/siglip2-base-patch16-224"
# Der lokale Ordner, in dem die PyTorch-Quelldateien gespeichert werden.
output_dir = "siglip2-base-patch16-224"


snapshot_download(
    repo_id=model_id,
    local_dir=output_dir,
    local_dir_use_symlinks=False,)
