//! Shared fragment length / ratio threshold logic.

/// Minimum fragment length considering absolute min and optional ratio of source text.
pub fn effective_min_fragment_len(
    source_len: usize,
    min_fragment_len: Option<usize>,
    min_fragment_ratio: Option<f64>,
) -> usize {
    let base = min_fragment_len.unwrap_or(8).max(1);
    if let Some(ratio) = min_fragment_ratio {
        let from_ratio = ((source_len as f64) * ratio.clamp(0.0, 1.0)).floor() as usize;
        return base.max(from_ratio).max(1);
    }
    base
}

/// Whether a fragment of `fragment_len` should be considered for matching against `source_len`.
pub fn fragment_meets_threshold(
    source_len: usize,
    fragment_len: usize,
    min_fragment_len: Option<usize>,
    min_fragment_ratio: Option<f64>,
) -> bool {
    if fragment_len == 0 || source_len == 0 {
        return false;
    }
    fragment_len >= effective_min_fragment_len(source_len, min_fragment_len, min_fragment_ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_increases_min_len() {
        assert_eq!(effective_min_fragment_len(100, Some(8), Some(0.1)), 10);
    }

    #[test]
    fn absolute_min_wins_when_larger() {
        assert_eq!(effective_min_fragment_len(50, Some(20), Some(0.1)), 20);
    }
}
