//! Picking one GPU when llama.cpp can see more than one.
//!
//! llama.cpp's default layer split spreads a model across *every* registered
//! device. That is right for a rack of matched cards and wrong for the machines
//! this runs on, where the extra devices are usually not extra capacity:
//!
//!   * A laptop reports its integrated GPU alongside the discrete one, so the
//!     default splits the model between a fast card and a slow one sharing
//!     system RAM — worse than using the discrete card on its own.
//!   * Should the Windows build ever enable CUDA next to Vulkan (see
//!     `Cargo.toml` for why it does not today), one NVIDIA card is registered
//!     *twice*, as `CUDA0` and as `Vulkan0`. llama.cpp does not deduplicate
//!     that, so the default would split a model across two views of a single
//!     card and count its VRAM twice.
//!
//! So we choose exactly one device and pass it to
//! `LlamaModelParams::with_devices`. The ranking already prefers CUDA over
//! Vulkan, which costs nothing while only Vulkan ships and means enabling CUDA
//! does not also mean rediscovering the duplicate-device problem.

/// What we need to know about a ggml backend device to rank it.
///
/// Mirrors the fields of `llama_cpp_2::LlamaBackendDevice` that matter here,
/// so the ranking can be exercised without a GPU (or a llama.cpp build).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Device {
    pub index: usize,
    /// Backend registry name, e.g. `"CUDA"`, `"Vulkan"`, `"Metal"`, `"CPU"`.
    pub backend: String,
    /// Human-readable, e.g. `"NVIDIA GeForce RTX 4070"`.
    pub description: String,
    pub kind: DeviceKind,
    pub memory_total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceKind {
    Cpu,
    Gpu,
    IntegratedGpu,
    Other,
}

/// Snapshot the ggml devices llama.cpp currently has registered.
///
/// Only meaningful after `LlamaBackend::init()`, which is what loads the
/// backend modules in the first place.
pub(crate) fn available() -> Vec<Device> {
    llama_cpp_2::list_llama_ggml_backend_devices()
        .into_iter()
        .map(|d| Device {
            index: d.index,
            backend: d.backend,
            description: d.description,
            kind: match d.device_type {
                llama_cpp_2::LlamaBackendDeviceType::Cpu => DeviceKind::Cpu,
                llama_cpp_2::LlamaBackendDeviceType::Gpu => DeviceKind::Gpu,
                llama_cpp_2::LlamaBackendDeviceType::IntegratedGpu => DeviceKind::IntegratedGpu,
                _ => DeviceKind::Other,
            },
            memory_total: d.memory_total,
        })
        .collect()
}

/// Preference score for a backend. Higher wins; `None` means "never pick".
fn backend_rank(backend: &str) -> Option<u8> {
    // Case-insensitive: the registry name casing is llama.cpp's business, not
    // a contract we should depend on.
    match backend.to_ascii_lowercase().as_str() {
        "cuda" => Some(4),
        "metal" => Some(3),
        "vulkan" => Some(2),
        // Anything else that presents as a GPU (ROCm, SYCL, OpenCL…) is still
        // better than falling back to CPU, but we would rather have a backend
        // we actually ship and test.
        _ => Some(1),
    }
}

/// Choose the GPU to run on, or `None` to let llama.cpp decide (CPU only).
///
/// CPU devices are never returned: llama.cpp always keeps the CPU backend
/// available for layers that stay off the GPU, and naming it explicitly here
/// would pin the whole model to it.
pub(crate) fn select_gpu(devices: &[Device]) -> Option<&Device> {
    devices
        .iter()
        .filter(|d| matches!(d.kind, DeviceKind::Gpu | DeviceKind::IntegratedGpu))
        .max_by_key(|d| {
            (
                backend_rank(&d.backend).unwrap_or(0),
                // Discrete before integrated, whatever the backend.
                u8::from(matches!(d.kind, DeviceKind::Gpu)),
                // Between two otherwise equal cards, take the roomier one.
                d.memory_total,
                // Deterministic tie-break so the choice is stable across runs;
                // max_by_key keeps the last maximum, so invert the index to
                // land on the first-registered device.
                usize::MAX - d.index,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(index: usize, backend: &str, kind: DeviceKind, memory_total: usize) -> Device {
        Device {
            index,
            backend: backend.to_string(),
            description: format!("{backend} device {index}"),
            kind,
            memory_total,
        }
    }

    const GB: usize = 1024 * 1024 * 1024;

    #[test]
    fn prefers_cuda_over_vulkan_for_the_same_card() {
        // Exactly what a Windows NVIDIA machine reports once both modules load.
        let devices = [
            dev(0, "CUDA", DeviceKind::Gpu, 12 * GB),
            dev(1, "Vulkan", DeviceKind::Gpu, 12 * GB),
            dev(2, "CPU", DeviceKind::Cpu, 32 * GB),
        ];
        let picked = select_gpu(&devices).expect("a GPU should be selected");
        assert_eq!(picked.backend, "CUDA");
        assert_eq!(picked.index, 0);
    }

    #[test]
    fn falls_back_to_vulkan_without_cuda() {
        let devices = [
            dev(0, "Vulkan", DeviceKind::Gpu, 8 * GB),
            dev(1, "CPU", DeviceKind::Cpu, 32 * GB),
        ];
        assert_eq!(select_gpu(&devices).unwrap().backend, "Vulkan");
    }

    #[test]
    fn prefers_discrete_over_integrated() {
        // Laptop with an Intel iGPU and a discrete card, both via Vulkan.
        let devices = [
            dev(0, "Vulkan", DeviceKind::IntegratedGpu, 2 * GB),
            dev(1, "Vulkan", DeviceKind::Gpu, 6 * GB),
        ];
        assert_eq!(select_gpu(&devices).unwrap().index, 1);
    }

    #[test]
    fn takes_the_larger_card_when_backends_tie() {
        let devices = [
            dev(0, "Vulkan", DeviceKind::Gpu, 4 * GB),
            dev(1, "Vulkan", DeviceKind::Gpu, 16 * GB),
        ];
        assert_eq!(select_gpu(&devices).unwrap().index, 1);
    }

    #[test]
    fn cuda_wins_even_against_a_larger_vulkan_device() {
        // The same 12 GB card seen twice must not be beaten by a bigger number
        // on the Vulkan side; and an 8 GB CUDA card is still the better pick
        // than a 12 GB Vulkan view of a *different* card would be, because the
        // backend gap dominates.
        let devices = [
            dev(0, "Vulkan", DeviceKind::Gpu, 16 * GB),
            dev(1, "CUDA", DeviceKind::Gpu, 8 * GB),
        ];
        assert_eq!(select_gpu(&devices).unwrap().backend, "CUDA");
    }

    #[test]
    fn integrated_gpu_is_still_better_than_nothing() {
        let devices = [
            dev(0, "Vulkan", DeviceKind::IntegratedGpu, 2 * GB),
            dev(1, "CPU", DeviceKind::Cpu, 16 * GB),
        ];
        assert_eq!(select_gpu(&devices).unwrap().index, 0);
    }

    #[test]
    fn cpu_only_machine_selects_nothing() {
        let devices = [dev(0, "CPU", DeviceKind::Cpu, 16 * GB)];
        assert!(select_gpu(&devices).is_none());
    }

    #[test]
    fn no_devices_selects_nothing() {
        assert!(select_gpu(&[]).is_none());
    }

    #[test]
    fn ties_resolve_to_the_first_registered_device() {
        let devices = [
            dev(0, "CUDA", DeviceKind::Gpu, 12 * GB),
            dev(1, "CUDA", DeviceKind::Gpu, 12 * GB),
        ];
        assert_eq!(select_gpu(&devices).unwrap().index, 0);
    }

    #[test]
    fn backend_name_casing_does_not_matter() {
        let devices = [
            dev(0, "vulkan", DeviceKind::Gpu, 12 * GB),
            dev(1, "cuda", DeviceKind::Gpu, 12 * GB),
        ];
        assert_eq!(select_gpu(&devices).unwrap().index, 1);
    }
}
