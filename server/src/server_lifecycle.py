import os
import time
import signal
from config import DB_PATH, logger, IMAGE_MODEL_ID, CLIP_MODEL_NAME, TORCH_DEVICE
import open_clip
from open_clip_compat import wrap_tokenizer
import threading
import datetime
import gc
import torch
from huggingface_hub import hf_hub_download
import service_face
import service_chroma


# Lazy-loadable global model instances
# model, processor and tokenizer start as None and will be loaded on first use.
model = None
processor = None  # This will hold the image preprocessor
tokenizer = None
_model_load_error = None

# Idle-unload handling
# If the model hasn't been used for this many seconds, it will be unloaded to free memory.
IDLE_UNLOAD_SECONDS = 30 * 60  # 30 minutes


_last_used = None
_model_lock = threading.RLock()
_unloader_thread = None


def _get_open_clip_tokenizer():
    """Resolve tokenizer across open_clip_torch and older forks."""
    try:
        return open_clip.get_tokenizer(IMAGE_MODEL_ID)
    except Exception:
        return open_clip.get_tokenizer(CLIP_MODEL_NAME)


def _set_last_used():
    global _last_used
    _last_used = datetime.datetime.utcnow()


def _needs_unload():
    if _last_used is None:
        return False
    delta = datetime.datetime.utcnow() - _last_used
    return delta.total_seconds() >= IDLE_UNLOAD_SECONDS


def is_model_cached() -> bool:
    """Check if the model is bundled or cached locally without downloading."""
    global model
    with _model_lock:
        if model is not None:
            return True

        try:
            cached_model_file = hf_hub_download(
                repo_id=IMAGE_MODEL_ID,
                filename="open_clip_model.safetensors",
                local_files_only=True,
            )
            cached_model_dir = os.path.dirname(cached_model_file)
            if os.path.isdir(cached_model_dir):
                config_file = os.path.join(cached_model_dir, "open_clip_config.json")
                weights_file = os.path.join(
                    cached_model_dir, "open_clip_model.safetensors"
                )
                if os.path.isfile(config_file) and os.path.isfile(weights_file):
                    return True
            return False
        except Exception:
            return False


def load_model():
    """Load the OpenCLIP model (idempotent)."""
    global model, processor, tokenizer
    with _model_lock:
        if model is not None:
            _set_last_used()
            return

        try:
            logger.info("Trying to load open_clip model from local cache")

            try:
                cached_model_file = hf_hub_download(
                    repo_id=IMAGE_MODEL_ID,
                    filename="open_clip_model.safetensors",
                    local_files_only=True,
                )

                cached_model_dir = os.path.dirname(cached_model_file)

                logger.info(f"Checking for cached model at: {cached_model_dir}")

                # Check if local model directory exists (production/bundled scenario)
                if os.path.isdir(cached_model_dir):
                    # Verify model files exist
                    config_file = os.path.join(
                        cached_model_dir, "open_clip_config.json"
                    )
                    weights_file = os.path.join(
                        cached_model_dir, "open_clip_model.safetensors"
                    )

                    if os.path.isfile(config_file) and os.path.isfile(weights_file):
                        logger.info(
                            f"Loading OpenCLIP model from cached directory: {cached_model_dir}"
                        )

                        # Preferred path for pip-installed open_clip_torch:
                        # use known architecture name + local checkpoint path.
                        try:
                            model_obj, _, proc = open_clip.create_model_and_transforms(
                                CLIP_MODEL_NAME,
                                pretrained=weights_file,
                            )
                            tok = _get_open_clip_tokenizer()
                        except Exception:
                            # Backward compatibility for older vendored open_clip forks.
                            local_model_uri = f"local-dir:{cached_model_dir}"
                            model_obj, _, proc = open_clip.create_model_and_transforms(
                                local_model_uri, pretrained=None
                            )
                            try:
                                tok = open_clip.get_tokenizer(local_model_uri)
                            except Exception:
                                tok = _get_open_clip_tokenizer()

                        _set_last_used()
                        logger.info("Loaded OpenCLIP model (lazy)")
                    else:
                        logger.warning(
                            "Bundled model directory exists but required files missing"
                        )
                        logger.warning(
                            f"Config file exists: {os.path.isfile(config_file)}"
                        )
                        logger.warning(
                            f"Weights file exists: {os.path.isfile(weights_file)}"
                        )
                        raise FileNotFoundError("Bundled model files incomplete")

                    try:
                        model_obj.to(TORCH_DEVICE)
                        logger.info(f"Text and vision model moved to {TORCH_DEVICE}")
                    except Exception as e:
                        logger.warning(
                            f"Failed to move text and vision model to {TORCH_DEVICE}: {e}."
                        )

                    model = model_obj
                    processor = proc
                    tokenizer = wrap_tokenizer(tok)

            except Exception as e:
                logger.warning(
                    f"Failed to load OpenCLIP model from local cache. This can happen if the model is not fully downloaded, is corrupted, or if there is a configuration issue. The error was: {e}",
                    exc_info=True,
                )
                logger.info("Falling back to loading OpenCLIP model via hf-hub")
                model_obj, _, proc = open_clip.create_model_and_transforms(
                    CLIP_MODEL_NAME,
                    pretrained=f"hf-hub:{IMAGE_MODEL_ID}",
                )
                tok = _get_open_clip_tokenizer()
                try:
                    model_obj.to(TORCH_DEVICE)
                    logger.info(f"Text and vision model moved to {TORCH_DEVICE}")
                except Exception as move_exc:
                    logger.warning(
                        f"Failed to move text and vision model to {TORCH_DEVICE}: {move_exc}."
                    )

                model = model_obj
                processor = proc
                tokenizer = wrap_tokenizer(tok)
                _set_last_used()
                logger.info("Loaded OpenCLIP model via hf-hub fallback")

        except Exception as e:
            _model_load_error = str(e)
            logger.error(f"Failed to load OpenCLIP model (lazy): {e}", exc_info=True)
            raise


def unload_model():
    """Unload the loaded model to free GPU/CPU memory."""
    global model, processor, tokenizer
    with _model_lock:
        if model is None and processor is None and tokenizer is None:
            return

        logger.info("Unloading OpenCLIP model due to inactivity...")
        try:
            # If the model is a torch module, try moving it to cpu and delete the reference.
            try:
                if hasattr(model, "to"):
                    model.to("cpu")
            except Exception:
                pass

            model = None
            processor = None
            tokenizer = None

            # Best-effort free memory for CUDA
            try:
                if torch.cuda.is_available():
                    torch.cuda.empty_cache()
            except Exception:
                pass

            # Force a GC pass
            gc.collect()
            logger.info("Unloaded OpenCLIP model.")
        except Exception as e:
            logger.warning(f"Error while unloading model: {e}")


def unload_all_resources():
    """Unload all heavy models and resources from memory."""
    logger.info("Unloading all resources (CLIP, InsightFace, ChromaDB)...")

    # 1. CLIP (OpenCLIP)
    try:
        unload_model()
    except Exception as e:
        logger.error(f"Failed to unload CLIP model: {e}")

    # 2. InsightFace
    try:
        service_face.unload_face_app()
    except Exception as e:
        logger.error(f"Failed to unload InsightFace model: {e}")

    # 3. ChromaDB
    try:
        service_chroma.unload_collections()
    except Exception as e:
        logger.error(f"Failed to unload ChromaDB: {e}")

    # 4. Final GC
    import gc

    gc.collect()
    logger.info("All resources unloaded successfully.")


def _idle_unloader_loop():
    """Background thread which periodically checks whether the model should be unloaded."""
    global _unloader_thread
    logger.info("Starting server_lifecycle idle unloader thread")
    try:
        while True:
            time.sleep(60)
            try:
                if _needs_unload():
                    unload_model()
            except Exception:
                logger.exception("Error checking/unloading model in background thread")
    finally:
        logger.info("Server_lifecycle idle unloader thread exiting")


def _ensure_unloader_thread():
    global _unloader_thread
    if _unloader_thread is None or not _unloader_thread.is_alive():
        _unloader_thread = threading.Thread(
            target=_idle_unloader_loop, daemon=True, name="server_lifecycle_unloader"
        )
        _unloader_thread.start()


def get_model():
    """Return the model, loading it lazily if needed."""
    load_model()
    _ensure_unloader_thread()
    return model


def get_processor():
    load_model()
    _ensure_unloader_thread()
    return processor


def get_tokenizer():
    load_model()
    _ensure_unloader_thread()
    return tokenizer


def get_db_dir():
    return os.path.dirname(DB_PATH) if DB_PATH else None


def write_pid_file():
    db_dir = get_db_dir()
    if not db_dir:
        return
    pid_file = os.path.join(db_dir, "lrgenius-server.pid")
    with open(pid_file, "w") as f:
        f.write(str(os.getpid()))


def remove_pid_file():
    db_dir = get_db_dir()
    if not db_dir:
        return
    pid_file = os.path.join(db_dir, "lrgenius-server.pid")
    try:
        os.remove(pid_file)
    except FileNotFoundError:
        pass


def write_ok_file():
    db_dir = get_db_dir()
    if not db_dir:
        return
    ok_file = os.path.join(db_dir, "lrgenius-server.OK")
    with open(ok_file, "w") as f:
        f.write("OK\n")


def remove_ok_file():
    db_dir = get_db_dir()
    if not db_dir:
        return
    ok_file = os.path.join(db_dir, "lrgenius-server.OK")
    try:
        os.remove(ok_file)
    except FileNotFoundError:
        pass


def request_shutdown():
    logger.info("Shutdown request received")
    time.sleep(1)  # Give time for the response to be sent
    os.kill(os.getpid(), signal.SIGINT)


def get_health_status():
    """Return health status of the model."""
    status = "not_loaded"
    if model is not None:
        status = "loaded"
    elif _model_load_error is not None:
        status = "failed"

    return {"clip_model": status, "clip_error": _model_load_error}
