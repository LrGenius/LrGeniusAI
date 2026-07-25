//! Port of `services/search.py::_smart_filter_by_relevance`: adaptive
//! per-provider relevance filter using a noise baseline (median + IQR of
//! the bottom half of results) plus a knee/gap detector that prefers a
//! clear distance break over the statistical cutoff when one exists.

pub const DEFAULT_MAX_RESULTS: i64 = 300;
pub const MAX_RESULTS_HARD_LIMIT: i64 = 2000;
pub const MIN_RESULTS_HARD_LIMIT: i64 = 10;
pub const DEFAULT_RELEVANCE_STRICTNESS: i64 = 50;
const RELEVANCE_FLOOR: usize = 8;
const RELEVANCE_KNEE_SCAN_LIMIT: usize = 100;
const RELEVANCE_KNEE_GAP_RATIO: f64 = 0.15;

pub fn clamp_max_results(value: Option<i64>) -> i64 {
    match value {
        None => DEFAULT_MAX_RESULTS,
        Some(n) => n.clamp(MIN_RESULTS_HARD_LIMIT, MAX_RESULTS_HARD_LIMIT),
    }
}

pub fn clamp_strictness(value: Option<i64>) -> i64 {
    value.unwrap_or(DEFAULT_RELEVANCE_STRICTNESS).clamp(0, 100)
}

/// Linear interpolation between anchor points: 25->0.5, 50->1.0, 75->1.5,
/// 100->2.0; 0 disables filtering.
fn strictness_to_k(strictness: i64) -> f64 {
    let s = strictness.clamp(0, 100);
    if s == 0 {
        return 0.0;
    }
    0.5 + (s - 25) as f64 * (1.5 / 75.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoredResult {
    pub photo_id: String,
    pub distance: f64,
}

/// `results` must already be sorted by distance ascending.
pub fn smart_filter_by_relevance(
    results: Vec<ScoredResult>,
    strictness: Option<i64>,
) -> Vec<ScoredResult> {
    if results.is_empty() {
        return results;
    }
    let s = clamp_strictness(strictness);
    if s == 0 {
        return results;
    }
    let n = results.len();
    if n <= RELEVANCE_FLOOR {
        return results;
    }

    let distances: Vec<f64> = results.iter().map(|r| r.distance).collect();

    let tail_start = n / 2;
    let mut sorted_tail: Vec<f64> = distances[tail_start..].to_vec();
    sorted_tail.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let m = sorted_tail.len();
    let noise_median = sorted_tail[m / 2];
    let q1 = sorted_tail[m / 4];
    let q3 = sorted_tail[(3 * m) / 4];
    let noise_spread = (q3 - q1).max(1e-6);

    let k = strictness_to_k(s);
    let stat_cutoff = noise_median - k * noise_spread;

    let scan_limit = RELEVANCE_KNEE_SCAN_LIMIT.min(n - 1);
    let mut knee_index: Option<usize> = None;
    let mut best_ratio = 0.0f64;
    for i in 0..scan_limit {
        let d_here = distances[i];
        let d_next = distances[i + 1];
        let ratio = (d_next - d_here) / d_here.max(1e-6);
        if ratio > best_ratio && ratio >= RELEVANCE_KNEE_GAP_RATIO {
            best_ratio = ratio;
            knee_index = Some(i + 1);
        }
    }

    let cutoff = match knee_index {
        Some(idx) => distances[idx - 1],
        None => stat_cutoff,
    };

    let mut kept: Vec<ScoredResult> = results
        .iter()
        .filter(|r| r.distance <= cutoff)
        .cloned()
        .collect();
    if kept.len() < RELEVANCE_FLOOR {
        kept = results.into_iter().take(RELEVANCE_FLOOR).collect();
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(distances: &[f64]) -> Vec<ScoredResult> {
        distances
            .iter()
            .enumerate()
            .map(|(i, &d)| ScoredResult {
                photo_id: format!("p{i}"),
                distance: d,
            })
            .collect()
    }

    #[test]
    fn strictness_zero_disables_filtering() {
        let r = results(&[0.1, 0.2, 0.9, 0.95, 0.96, 0.97, 0.98, 0.99, 1.0, 1.0]);
        let kept = smart_filter_by_relevance(r.clone(), Some(0));
        assert_eq!(kept, r);
    }

    #[test]
    fn small_result_sets_are_never_filtered() {
        let r = results(&[0.1, 0.2, 0.9, 0.95, 0.96, 0.97, 0.98, 0.99]); // exactly the floor
        let kept = smart_filter_by_relevance(r.clone(), Some(80));
        assert_eq!(kept, r);
    }

    #[test]
    fn clear_gap_triggers_knee_cutoff() {
        // Ten very close matches, then a big jump to noise. Kept set (10)
        // must exceed RELEVANCE_FLOOR (8) so the floor override doesn't
        // mask the knee behavior being tested here.
        let mut distances: Vec<f64> = (0..10).map(|i| 0.05 + i as f64 * 0.001).collect();
        for i in 0..10 {
            distances.push(0.5 + i as f64 * 0.001);
        }
        let r = results(&distances);
        let kept = smart_filter_by_relevance(r, Some(50));
        assert_eq!(
            kept.len(),
            10,
            "knee should cut right after the ten close matches"
        );
    }

    #[test]
    fn tiny_kept_set_falls_back_to_the_floor() {
        // Only 2 results are actually relevant, but the floor guarantees
        // at least RELEVANCE_FLOOR results come back regardless.
        let mut distances = vec![0.05, 0.06];
        for i in 0..10 {
            distances.push(0.5 + i as f64 * 0.001);
        }
        let r = results(&distances);
        let kept = smart_filter_by_relevance(r, Some(50));
        assert_eq!(kept.len(), RELEVANCE_FLOOR);
    }

    #[test]
    fn floor_guarantees_minimum_results_on_smooth_distribution() {
        // Smooth, gradually increasing distances with no gap and a high
        // strictness — the statistical cutoff could drop below the floor.
        let distances: Vec<f64> = (0..20).map(|i| 0.1 + i as f64 * 0.01).collect();
        let r = results(&distances);
        let kept = smart_filter_by_relevance(r, Some(100));
        assert!(kept.len() >= RELEVANCE_FLOOR, "kept {} < floor", kept.len());
    }

    #[test]
    fn clamp_helpers_match_python_bounds() {
        assert_eq!(clamp_max_results(None), 300);
        assert_eq!(clamp_max_results(Some(1)), 10);
        assert_eq!(clamp_max_results(Some(100_000)), 2000);
        assert_eq!(clamp_max_results(Some(500)), 500);
        assert_eq!(clamp_strictness(None), 50);
        assert_eq!(clamp_strictness(Some(-10)), 0);
        assert_eq!(clamp_strictness(Some(200)), 100);
    }
}
