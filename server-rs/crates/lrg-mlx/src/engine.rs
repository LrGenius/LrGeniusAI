//! The public handle: spawns the sidecar, loads a model into it, and serves
//! generation requests over its stdio protocol.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::protocol::{GenerationSpec, ImagePayload, Request, Response};
use crate::{GenerationOutput, GenerationRequest, MlxError};

/// Name of the sidecar binary, as produced by `swift build` and as staged next
/// to `lrgenius-server` by the installer.
const SIDECAR_BIN: &str = "lrgenius-mlx";

#[derive(Debug, Clone)]
pub struct MlxEngineConfig {
    /// Directory holding an MLX model: `config.json`, the safetensors shards
    /// and the tokenizer files. Unlike GGUF this is a directory, not a file.
    pub model_dir: PathBuf,
    /// Explicit sidecar path. `None` searches the usual places; see
    /// [`resolve_sidecar`].
    pub sidecar_path: Option<PathBuf>,
}

/// What the sidecar reported after loading, for `/llm/status` and the logs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EngineInfo {
    pub model_path: String,
    pub model_name: String,
    pub supports_vision: bool,
}

/// The live connection. Held behind a mutex so that one request completes
/// before the next is written — the sidecar serves strictly in order.
///
/// `stdin` is an `Option` purely so [`Drop`] can take it: dropping the handle
/// is what closes the pipe, and closing the pipe is how the sidecar is told to
/// exit. There is no way to swap a `ChildStdin` for a placeholder.
struct Connection {
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    child: Child,
}

impl Connection {
    /// The write half, or a clear error if [`Drop`] has already closed it.
    fn stdin(&mut self) -> Result<&mut ChildStdin, MlxError> {
        self.stdin
            .as_mut()
            .ok_or_else(|| MlxError::Sidecar("the sidecar connection is closed".to_string()))
    }
}

pub struct MlxEngine {
    connection: Mutex<Connection>,
    next_id: AtomicU64,
    info: EngineInfo,
}

impl MlxEngine {
    /// Spawn the sidecar and load `config.model_dir` into it.
    ///
    /// Loading reads multi-gigabyte weights, so this is slow; callers should
    /// treat it like `LlamaEngine::new` and keep it off a hot path. It is
    /// `async` rather than blocking because all the waiting is on a pipe.
    ///
    /// # Errors
    /// Reports a missing sidecar, a missing model directory, a non-Apple-silicon
    /// host, or whatever the sidecar said went wrong while loading.
    pub async fn new(config: MlxEngineConfig) -> Result<Self, MlxError> {
        if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            return Err(MlxError::UnsupportedPlatform);
        }
        if !config.model_dir.is_dir() {
            return Err(MlxError::ModelNotFound(
                config.model_dir.display().to_string(),
            ));
        }

        let sidecar = match config.sidecar_path {
            Some(path) if path.is_file() => path,
            Some(path) => {
                return Err(MlxError::SidecarMissing(format!(
                    "LRG_MLX_SIDECAR points at {}, which is not a file",
                    path.display()
                )))
            }
            None => resolve_sidecar()?,
        };

        log::info!("Starting MLX sidecar: {}", sidecar.display());
        let mut child = tokio::process::Command::new(&sidecar)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr is deliberately inherited rather than piped: the sidecar
            // logs there, and inheriting puts those lines straight into the
            // server's own stderr (and so its log file) without this crate
            // having to pump a third pipe just to avoid filling its buffer.
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                MlxError::SidecarMissing(format!("could not start {}: {e}", sidecar.display()))
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| MlxError::Sidecar("sidecar stdin was not piped".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| MlxError::Sidecar("sidecar stdout was not piped".to_string()))?;

        let mut engine = MlxEngine {
            connection: Mutex::new(Connection {
                stdin: Some(stdin),
                stdout: BufReader::new(stdout),
                child,
            }),
            next_id: AtomicU64::new(1),
            info: EngineInfo {
                model_path: config.model_dir.display().to_string(),
                model_name: model_name_of(&config.model_dir),
                supports_vision: false,
            },
        };

        let model_dir = config.model_dir.display().to_string();
        let response = engine
            .call(Request {
                id: 0, // replaced by `call`
                op: "load",
                model_dir: Some(&model_dir),
                requests: None,
            })
            .await?;

        if !response.ok {
            return Err(MlxError::Load(response.error.unwrap_or_else(|| {
                "the MLX sidecar could not load the model".to_string()
            })));
        }
        let info = response
            .info
            .ok_or_else(|| MlxError::Load("the MLX sidecar reported no model info".to_string()))?;

        // Assigned rather than built with struct-update syntax: `MlxEngine`
        // implements `Drop`, so it cannot be partially moved out of.
        engine.info = EngineInfo {
            model_path: info.model_path,
            model_name: info.model_name,
            supports_vision: info.supports_vision,
        };
        log::info!(
            "MLX engine ready: model={} vision={}",
            engine.info.model_name,
            engine.info.supports_vision
        );

        Ok(engine)
    }

    #[must_use]
    pub fn info(&self) -> &EngineInfo {
        &self.info
    }

    /// Generate for a batch of photos.
    ///
    /// Each entry gets its own `Result` so one undecodable photo does not sink
    /// the rest. Unlike the llama.cpp backend the batch is not a unit of
    /// parallel decoding — see the module docs — but it is still the right call
    /// shape, because it keeps one round-trip per group rather than per photo.
    ///
    /// # Errors
    /// Returns `Err` only when the sidecar itself is gone; per-photo failures
    /// are reported inside the returned `Vec`.
    pub async fn generate(
        &self,
        requests: Vec<GenerationRequest>,
    ) -> Result<Vec<Result<GenerationOutput, MlxError>>, MlxError> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        // Pixels are staged on disk; the guards delete them when this call
        // returns, whether it succeeded or not.
        let mut staged = Vec::new();
        let mut specs = Vec::with_capacity(requests.len());
        for request in requests {
            let image = match request.image {
                Some(image) => {
                    let file = StagedImage::write(&image)?;
                    let payload = ImagePayload {
                        path: file.path.display().to_string(),
                        width: image.width,
                        height: image.height,
                        channels: 3,
                    };
                    staged.push(file);
                    Some(payload)
                }
                None => None,
            };
            specs.push(GenerationSpec {
                system_prompt: request.system_prompt,
                stable_prompt: request.stable_prompt,
                per_photo_prompt: request.per_photo_prompt,
                image,
                // Serialize here so the sidecar's grammar cache keys on stable
                // text: `serde_json` orders object keys deterministically for a
                // given `Value`, so the same schema yields the same string.
                schema: request.schema.as_ref().map(ToString::to_string),
                max_tokens: request.max_tokens,
                temperature: request.temperature,
            });
        }

        let count = specs.len();
        let response = self
            .call(Request {
                id: 0,
                op: "generate",
                model_dir: None,
                requests: Some(specs),
            })
            .await?;
        drop(staged);

        if !response.ok {
            return Err(MlxError::Inference(response.error.unwrap_or_else(|| {
                "the MLX sidecar reported a failure".to_string()
            })));
        }
        let results = response
            .results
            .ok_or_else(|| MlxError::Sidecar("generate returned no results".to_string()))?;
        if results.len() != count {
            return Err(MlxError::Sidecar(format!(
                "asked the MLX sidecar for {count} results and got {}",
                results.len()
            )));
        }

        Ok(results
            .into_iter()
            .map(|result| {
                if result.ok {
                    Ok(GenerationOutput {
                        text: result.text.unwrap_or_default(),
                        prompt_tokens: result.prompt_tokens.unwrap_or(0),
                        completion_tokens: result.completion_tokens.unwrap_or(0),
                        prefix_reused: result.prefix_reused,
                    })
                } else {
                    Err(MlxError::Inference(result.error.unwrap_or_else(|| {
                        "the MLX model failed on this photo".to_string()
                    })))
                }
            })
            .collect())
    }

    /// Write one request and read replies until the matching id comes back.
    ///
    /// Holding the connection lock for the whole exchange is what makes the
    /// correlation safe: no other request can interleave, so any line with a
    /// different id is a protocol bug rather than someone else's answer.
    async fn call(&self, mut request: Request<'_>) -> Result<Response, MlxError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        request.id = id;
        let mut line = serde_json::to_string(&request)
            .map_err(|e| MlxError::Sidecar(format!("could not encode request: {e}")))?;
        line.push('\n');

        let mut connection = self.connection.lock().await;
        let stdin = connection.stdin()?;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| MlxError::Sidecar(format!("writing to the sidecar failed: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| MlxError::Sidecar(format!("flushing to the sidecar failed: {e}")))?;

        loop {
            let mut reply = String::new();
            let read =
                connection.stdout.read_line(&mut reply).await.map_err(|e| {
                    MlxError::Sidecar(format!("reading from the sidecar failed: {e}"))
                })?;
            if read == 0 {
                // EOF: the sidecar exited. Reap it so the exit status makes it
                // into the log instead of leaving a zombie.
                let status = connection.child.try_wait().ok().flatten();
                return Err(MlxError::Sidecar(match status {
                    Some(status) => format!("the sidecar exited ({status})"),
                    None => "the sidecar closed its output".to_string(),
                }));
            }
            if reply.trim().is_empty() {
                continue;
            }
            let response: Response = serde_json::from_str(reply.trim()).map_err(|e| {
                MlxError::Sidecar(format!(
                    "could not decode the sidecar's reply ({e}): {}",
                    reply.trim().chars().take(200).collect::<String>()
                ))
            })?;
            if response.id == id {
                return Ok(response);
            }
            log::warn!(
                "Ignoring an MLX sidecar reply for request {} while awaiting {id}",
                response.id
            );
        }
    }
}

impl Drop for MlxEngine {
    fn drop(&mut self) {
        // Closing stdin is the shutdown signal: the sidecar's read loop sees
        // EOF and returns. Then kill unconditionally as a backstop, because a
        // process wedged inside a Metal decode will not reach its read loop,
        // and leaving it alive would hold gigabytes of weights resident long
        // after the model was supposedly unloaded.
        let connection = self.connection.get_mut();
        drop(connection.stdin.take());
        if let Err(e) = connection.child.start_kill() {
            log::debug!("MLX sidecar was already gone: {e}");
        }
    }
}

/// Raw pixels written where the sidecar can read them, deleted on drop.
struct StagedImage {
    path: PathBuf,
}

impl StagedImage {
    fn write(image: &crate::RgbImage) -> Result<Self, MlxError> {
        let path = std::env::temp_dir().join(format!("lrg-mlx-{}.rgb", uuid::Uuid::new_v4()));
        std::fs::write(&path, &image.data)
            .map_err(|e| MlxError::Image(format!("could not stage pixels at {path:?}: {e}")))?;
        Ok(StagedImage { path })
    }
}

impl Drop for StagedImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn model_name_of(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string())
}

/// Find the sidecar binary.
///
/// In order: the `LRG_MLX_SIDECAR` override, next to the running server (how
/// the installer stages it), then the SwiftPM build directory relative to the
/// repo root (how a dev build finds it without an install step).
///
/// # Errors
/// Names every path tried, since "not found" with no list is the least useful
/// possible message for this particular failure.
pub fn resolve_sidecar() -> Result<PathBuf, MlxError> {
    let mut tried = Vec::new();

    if let Some(explicit) = std::env::var_os("LRG_MLX_SIDECAR") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        tried.push(path);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join(SIDECAR_BIN);
            if beside.is_file() {
                return Ok(beside);
            }
            tried.push(beside);

            // A cargo dev build lives at server-rs/target/<profile>/, so the
            // sidecar's build directory is three levels up plus native/.
            //
            // The xcodebuild output comes first because it is the only one
            // that runs: `swift build` cannot compile MLX's Metal shaders, so
            // the binary it produces dies at load with "Failed to load the
            // default metallib". Its path is still checked second, because a
            // stale one being found is better than no sidecar at all — it
            // fails with a message naming metallib rather than one naming
            // nothing.
            let roots = dir.ancestors().nth(3).map(|root| {
                [
                    root.join("native/mlx-sidecar/.build/xcode/Build/Products/Release")
                        .join(SIDECAR_BIN),
                    root.join("native/mlx-sidecar/.build/release")
                        .join(SIDECAR_BIN),
                ]
            });
            for candidate in roots.into_iter().flatten() {
                if candidate.is_file() {
                    return Ok(candidate);
                }
                tried.push(candidate);
            }
        }
    }

    Err(MlxError::SidecarMissing(format!(
        "The MLX helper '{SIDECAR_BIN}' was not found. Looked in: {}. Build it with \
         `cd native/mlx-sidecar && xcodebuild build -scheme lrgenius-mlx -destination \
         'platform=macOS,arch=arm64' -configuration Release -derivedDataPath .build/xcode` \
         (plain `swift build` cannot compile the Metal shaders), or set LRG_MLX_SIDECAR.",
        tried
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_sidecar_names_what_was_tried() {
        std::env::set_var("LRG_MLX_SIDECAR", "/definitely/not/here/lrgenius-mlx");
        let err = resolve_sidecar().expect_err("a bogus override must not resolve");
        let message = err.to_string();
        std::env::remove_var("LRG_MLX_SIDECAR");
        assert!(message.contains("/definitely/not/here/lrgenius-mlx"));
        assert!(
            message.contains("swift build"),
            "the message must say how to fix it"
        );
    }

    #[tokio::test]
    async fn a_missing_model_directory_fails_before_spawning() {
        // `err()` rather than `expect_err()`: `MlxEngine` owns a child process
        // and deliberately does not implement `Debug`.
        let err = MlxEngine::new(MlxEngineConfig {
            model_dir: PathBuf::from("/no/such/model/dir"),
            sidecar_path: None,
        })
        .await
        .err()
        .expect("a missing model directory must fail");
        // On Apple silicon this is ModelNotFound; elsewhere the platform check
        // fires first. Both are correct, and both must beat "sidecar missing".
        assert!(matches!(
            err,
            MlxError::ModelNotFound(_) | MlxError::UnsupportedPlatform
        ));
    }

    #[test]
    fn model_name_is_the_directory_leaf() {
        assert_eq!(
            model_name_of(Path::new("/models/mlx/gemma-4-e4b-it-4bit")),
            "gemma-4-e4b-it-4bit"
        );
    }
}
