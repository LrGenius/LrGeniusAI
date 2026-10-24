//! What this computer is, and which local AI model suits it.
//!
//! The backend runs the local model, so it is the backend's computer that has
//! to fit it — which is also the right answer for a plug-in talking to a
//! backend on another machine. Reported by `/v1/llm/catalog` (`hardware`, and
//! a `recommended` model per engine), which the plug-in shows as
//! "Recommended for this computer" in the setup wizard and the Plug-in
//! Manager.
//!
//! # The recommendation
//!
//! One preference list per engine, best model first ([`MLX_PREFERENCE`],
//! [`LLAMACPP_PREFERENCE`]). The recommendation is the first entry whose
//! `min_ram_gb` — the RAM floor its catalog already declares, so there is one
//! number per model, not two — the computer meets. Memory is the decision
//! because it is what makes a local model fail outright: Lightroom and the
//! system need their share of the same RAM, and a model that does not fit
//! next to them swaps until it is unusable (#341).
//!
//! What is deliberately *not* used:
//!
//! - **The GPU on Windows and Linux.** llama.cpp only learns which devices it
//!   has when its backend initialises, which loads the Vulkan modules — not a
//!   cheap probe for a settings page. Without knowing there is a discrete GPU,
//!   the llama.cpp list stops at Gemma 4 E4B: Gemma 4 12B on the CPU alone is
//!   too slow for a catalog, however much RAM there is. `gpu` is reported as
//!   unknown rather than guessed.
//! - **Apple silicon generation and tier.** They decide how fast a model runs,
//!   not whether it fits, and no throughput has been measured per chip yet
//!   (`examples/bench_local_backends.rs` is the tool for that). They are
//!   reported, and named in the reason, so the table can grow a speed rule
//!   once there are numbers behind one.

use std::sync::OnceLock;

use serde::Serialize;

/// MLX models, best first. Catalog ids from [`crate::mlx_models::CATALOG`].
pub const MLX_PREFERENCE: &[&str] = &["mlx-gemma4-12b", "mlx-gemma4-e4b", "mlx-gemma4-e2b"];

/// llama.cpp models, best first. Catalog ids from [`crate::llm_models::CATALOG`].
/// Stops at E4B on purpose — see the module docs on the GPU.
pub const LLAMACPP_PREFERENCE: &[&str] = &["gemma4-e4b", "qwen2.5-vl-3b", "smolvlm-500m"];

/// An Apple silicon chip, read from the CPU brand string ("Apple M2 Pro").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppleChip {
    /// "M2 Pro".
    pub name: String,
    /// 2 for an M2 of any tier; `None` when the brand names no number.
    pub generation: Option<u32>,
    /// "base", "pro", "max" or "ultra".
    pub tier: &'static str,
}

/// What `/v1/llm/catalog` reports about the computer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HardwareInfo {
    /// Physical memory the OS reports, in bytes.
    pub memory_bytes: u64,
    /// The memory size the computer was sold with, in GB ([`nominal_memory_gb`]).
    pub memory_gb: u32,
    /// CPU brand string, e.g. "Apple M2 Pro" or "AMD Ryzen 7 7840HS".
    pub cpu: String,
    pub logical_cores: usize,
    pub physical_cores: Option<usize>,
    /// `std::env::consts::OS`: "macos", "windows", "linux".
    pub os: &'static str,
    /// `std::env::consts::ARCH`: "aarch64", "x86_64".
    pub arch: &'static str,
    pub apple_chip: Option<AppleChip>,
    /// Whether the GPU shares the system memory (Apple silicon). A model's
    /// RAM floor then covers the GPU too.
    pub unified_memory: bool,
}

/// Memory sizes computers are sold with, in GB.
const STANDARD_SIZES_GB: &[u32] = &[
    4, 6, 8, 12, 16, 18, 24, 32, 36, 48, 64, 96, 128, 192, 256, 512,
];

/// The memory size a computer was sold with, from what the OS reports.
///
/// The OS reports less than is installed — firmware, and on a PC an
/// integrated GPU, keep a slice — so a 16 GB laptop can report 13.6 GiB and
/// would miss a 16 GB floor by a technicality. The nearest standard size at
/// or above the reported amount counts when the report is within 16 % of it;
/// anything else (a VM, an unusual configuration) is rounded as reported.
#[must_use]
pub fn nominal_memory_gb(bytes: u64) -> u32 {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    STANDARD_SIZES_GB
        .iter()
        .copied()
        .find(|&size| gib <= f64::from(size) + 0.01 && gib >= f64::from(size) * 0.84)
        .unwrap_or_else(|| gib.round() as u32)
}

/// The Apple silicon chip a CPU brand string names, if any.
#[must_use]
pub fn parse_apple_chip(brand: &str) -> Option<AppleChip> {
    let rest = brand.trim().strip_prefix("Apple ")?.trim();
    let first = rest.split_whitespace().next()?;
    let digits = first.strip_prefix('M')?;
    let generation = if digits.is_empty() {
        None
    } else {
        Some(digits.parse::<u32>().ok()?)
    };
    let tier = match rest
        .split_whitespace()
        .nth(1)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None => "base",
        Some("pro") => "pro",
        Some("max") => "max",
        Some("ultra") => "ultra",
        Some(_) => "base",
    };
    Some(AppleChip {
        name: rest.to_string(),
        generation,
        tier,
    })
}

/// Reads the facts once. Neither the memory nor the CPU changes while the
/// backend runs, and the catalog is asked for on every settings refresh.
pub fn detect() -> &'static HardwareInfo {
    static INFO: OnceLock<HardwareInfo> = OnceLock::new();
    INFO.get_or_init(|| {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
        let cpu = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        let memory_bytes = sys.total_memory();
        let apple_chip = parse_apple_chip(&cpu);
        let info = HardwareInfo {
            memory_bytes,
            memory_gb: nominal_memory_gb(memory_bytes),
            logical_cores: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or_else(|_| sys.cpus().len().max(1)),
            physical_cores: sysinfo::System::physical_core_count(),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            unified_memory: apple_chip.is_some(),
            apple_chip,
            cpu,
        };
        log::info!(
            "Hardware: {} GB memory ({} bytes), {} ({} logical cores), {}/{}",
            info.memory_gb,
            info.memory_bytes,
            info.cpu,
            info.logical_cores,
            info.os,
            info.arch
        );
        info
    })
}

/// A model from one engine's catalog, as the recommendation sees it.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub id: &'a str,
    pub label: &'a str,
    pub min_ram_gb: u32,
}

/// The model recommended for this computer on one engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Recommendation {
    /// Catalog id; `None` when the computer has too little memory for any.
    pub id: Option<String>,
    pub label: Option<String>,
    /// One sentence for the user: why this model, or why none.
    pub reason: String,
}

/// The label without the parenthesised note the catalogs add for the
/// dropdown ("Gemma 4 E4B (balanced)" → "Gemma 4 E4B").
fn short_label(label: &str) -> &str {
    label.split(" (").next().unwrap_or(label).trim()
}

/// What the reason calls this computer: the chip on a Mac, nothing elsewhere
/// (a PC's CPU brand says little about running a model).
fn describe(hw: &HardwareInfo) -> String {
    match &hw.apple_chip {
        Some(chip) => format!("{} GB of memory and an Apple {}", hw.memory_gb, chip.name),
        None => format!("{} GB of memory", hw.memory_gb),
    }
}

/// The first model of `preference` whose RAM floor `hw` meets.
///
/// `catalog` is the engine's whole catalog; ids in `preference` that it does
/// not hold are skipped (a test keeps them in step).
#[must_use]
pub fn recommend(
    hw: &HardwareInfo,
    preference: &[&str],
    catalog: &[Candidate<'_>],
) -> Recommendation {
    let pick = preference
        .iter()
        .filter_map(|id| catalog.iter().find(|c| c.id == *id))
        .find(|c| hw.memory_gb >= c.min_ram_gb);
    match pick {
        Some(c) => Recommendation {
            id: Some(c.id.to_string()),
            label: Some(c.label.to_string()),
            reason: format!(
                "{} is the best model that fits next to Lightroom on a computer with {}.",
                short_label(c.label),
                describe(hw)
            ),
        },
        None => {
            let smallest = preference
                .iter()
                .filter_map(|id| catalog.iter().find(|c| c.id == *id))
                .map(|c| c.min_ram_gb)
                .min()
                .unwrap_or(0);
            Recommendation {
                id: None,
                label: None,
                reason: format!(
                    "This computer has {}; the smallest local model needs {smallest} GB next to \
                     Lightroom. Use a cloud provider, or Ollama or LM Studio on a computer with \
                     more memory.",
                    describe(hw)
                ),
            }
        }
    }
}

/// The MLX catalog as candidates.
#[must_use]
pub fn mlx_candidates() -> Vec<Candidate<'static>> {
    crate::mlx_models::CATALOG
        .iter()
        .map(|e| Candidate {
            id: e.id,
            label: e.label,
            min_ram_gb: e.min_ram_gb,
        })
        .collect()
}

/// The llama.cpp catalog as candidates.
#[must_use]
pub fn llamacpp_candidates() -> Vec<Candidate<'static>> {
    crate::llm_models::CATALOG
        .iter()
        .map(|e| Candidate {
            id: e.id,
            label: e.label,
            min_ram_gb: e.min_ram_gb,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn machine(memory_gb: u32, brand: &str) -> HardwareInfo {
        let apple_chip = parse_apple_chip(brand);
        HardwareInfo {
            memory_bytes: u64::from(memory_gb) * GIB,
            memory_gb,
            cpu: brand.to_string(),
            logical_cores: 8,
            physical_cores: Some(8),
            os: "macos",
            arch: "aarch64",
            unified_memory: apple_chip.is_some(),
            apple_chip,
        }
    }

    fn mlx_for(memory_gb: u32) -> Option<String> {
        recommend(
            &machine(memory_gb, "Apple M2"),
            MLX_PREFERENCE,
            &mlx_candidates(),
        )
        .id
    }

    fn llama_for(memory_gb: u32) -> Option<String> {
        recommend(
            &machine(memory_gb, "AMD Ryzen 7 7840HS"),
            LLAMACPP_PREFERENCE,
            &llamacpp_candidates(),
        )
        .id
    }

    /// The table, spelled out for the memory sizes Macs are sold with.
    #[test]
    fn mlx_recommendation_by_memory() {
        assert_eq!(mlx_for(4), None);
        assert_eq!(mlx_for(8).as_deref(), Some("mlx-gemma4-e2b"));
        assert_eq!(mlx_for(16).as_deref(), Some("mlx-gemma4-e4b"));
        assert_eq!(mlx_for(18).as_deref(), Some("mlx-gemma4-e4b"));
        assert_eq!(mlx_for(24).as_deref(), Some("mlx-gemma4-e4b"));
        assert_eq!(mlx_for(32).as_deref(), Some("mlx-gemma4-12b"));
        assert_eq!(mlx_for(128).as_deref(), Some("mlx-gemma4-12b"));
    }

    /// And for PCs, where the GPU is unknown and the list stops at E4B.
    #[test]
    fn llamacpp_recommendation_by_memory() {
        assert_eq!(llama_for(2), None);
        assert_eq!(llama_for(4).as_deref(), Some("smolvlm-500m"));
        assert_eq!(llama_for(8).as_deref(), Some("qwen2.5-vl-3b"));
        assert_eq!(llama_for(12).as_deref(), Some("qwen2.5-vl-3b"));
        assert_eq!(llama_for(16).as_deref(), Some("gemma4-e4b"));
        assert_eq!(llama_for(64).as_deref(), Some("gemma4-e4b"));
    }

    /// Every id in the preference lists is a real catalog entry, and each list
    /// runs from the largest RAM floor down — or the first-fit rule would
    /// recommend a smaller model on a bigger machine.
    #[test]
    fn preference_lists_name_catalog_models_best_first() {
        for (preference, catalog) in [
            (MLX_PREFERENCE, mlx_candidates()),
            (LLAMACPP_PREFERENCE, llamacpp_candidates()),
        ] {
            let floors: Vec<u32> = preference
                .iter()
                .map(|id| {
                    catalog
                        .iter()
                        .find(|c| c.id == *id)
                        .unwrap_or_else(|| panic!("{id} is not in its catalog"))
                        .min_ram_gb
                })
                .collect();
            assert!(
                floors.windows(2).all(|w| w[0] >= w[1]),
                "RAM floors must not rise down the list: {floors:?}"
            );
        }
    }

    #[test]
    fn the_reason_names_the_model_the_memory_and_the_chip() {
        let rec = recommend(
            &machine(16, "Apple M2 Pro"),
            MLX_PREFERENCE,
            &mlx_candidates(),
        );
        assert_eq!(
            rec.reason,
            "Gemma 4 E4B is the best model that fits next to Lightroom on a computer with 16 GB \
             of memory and an Apple M2 Pro."
        );
        let none = recommend(
            &machine(4, "Intel(R) Core(TM) i5"),
            MLX_PREFERENCE,
            &mlx_candidates(),
        );
        assert!(none.reason.contains("4 GB of memory"), "{}", none.reason);
        assert!(none.reason.contains("needs 8 GB"), "{}", none.reason);
        assert!(none.reason.contains("cloud provider"), "{}", none.reason);
    }

    #[test]
    fn reported_memory_maps_to_the_size_the_computer_was_sold_with() {
        // A Mac reports exactly what is installed.
        assert_eq!(nominal_memory_gb(16 * GIB), 16);
        assert_eq!(nominal_memory_gb(18 * GIB), 18);
        // A PC keeps a slice for firmware or an integrated GPU.
        assert_eq!(nominal_memory_gb(16 * GIB - GIB / 2), 16);
        assert_eq!(nominal_memory_gb((13.6 * GIB as f64) as u64), 16);
        assert_eq!(nominal_memory_gb((7.1 * GIB as f64) as u64), 8);
        assert_eq!(nominal_memory_gb((31.2 * GIB as f64) as u64), 32);
        // Far below every standard size near it: rounded as reported.
        assert_eq!(nominal_memory_gb((10.0 * GIB as f64) as u64), 10);
        assert_eq!(nominal_memory_gb(0), 0);
    }

    #[test]
    fn apple_chips_are_read_from_the_brand_string() {
        assert_eq!(
            parse_apple_chip("Apple M2 Pro"),
            Some(AppleChip {
                name: "M2 Pro".into(),
                generation: Some(2),
                tier: "pro"
            })
        );
        let m1 = parse_apple_chip("Apple M1").unwrap();
        assert_eq!((m1.generation, m1.tier), (Some(1), "base"));
        let ultra = parse_apple_chip("Apple M3 Ultra").unwrap();
        assert_eq!((ultra.generation, ultra.tier), (Some(3), "ultra"));
        assert_eq!(parse_apple_chip("Apple M4 Max").unwrap().tier, "max");
        assert_eq!(parse_apple_chip("Intel(R) Core(TM) i7-9750H"), None);
        assert_eq!(parse_apple_chip("AMD Ryzen 7 7840HS"), None);
        assert_eq!(parse_apple_chip("Apple processor"), None);
    }

    /// The real probe on whatever machine runs the tests: it must report
    /// something plausible rather than zeros.
    #[test]
    fn detection_reports_this_machine() {
        let hw = detect();
        assert!(hw.memory_bytes > 0);
        assert!(hw.memory_gb > 0);
        assert!(hw.logical_cores > 0);
        assert!(!hw.cpu.is_empty());
    }
}
