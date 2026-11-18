//! Gate classification of exact confidence sets (translated from the
//! in-crate unit tests of src/signed_rank_confidence.rs, so they run against
//! both builds). The classifiers are `#[doc(hidden)]` public functions.
mod paired_gate_tests {
    use std::cmp::Ordering;
    use vesper_bench::*;

    fn segment(
        lower: Option<i128>,
        lower_inclusive: bool,
        upper: Option<i128>,
        upper_inclusive: bool,
    ) -> SignedRankConfidenceSegment {
        SignedRankConfidenceSegment {
            lower_times_two: lower,
            lower_inclusive,
            upper_times_two: upper,
            upper_inclusive,
        }
    }

    #[test]
    fn compare_thresholds_respects_fractional_open_and_disconnected_sets() {
        assert_eq!(
            classify_paired_sample_set(&[], 2, 0),
            (
                Decision::Inconclusive,
                Some(PairedSampleGateInconclusive::EmptyConfidenceSet)
            )
        );
        let right_open = [segment(Some(0), false, Some(2), true)];
        assert_eq!(
            classify_paired_sample_set(&right_open, 2, 0),
            (Decision::Regression, None)
        );
        let right_closed = [segment(Some(0), true, Some(2), true)];
        assert_eq!(
            classify_paired_sample_set(&right_closed, 2, 0),
            (
                Decision::Inconclusive,
                Some(PairedSampleGateInconclusive::ThresholdNotSeparated)
            )
        );
        let left_open = [segment(None, false, Some(0), false)];
        assert_eq!(
            classify_paired_sample_set(&left_open, 2, 0),
            (Decision::Pass, None)
        );
        let disconnected = [
            segment(Some(-4), true, Some(-2), true),
            segment(Some(2), true, Some(4), true),
        ];
        assert_eq!(
            classify_paired_sample_set(&disconnected, 2, 0),
            (
                Decision::Inconclusive,
                Some(PairedSampleGateInconclusive::ThresholdNotSeparated)
            )
        );
        let all = [segment(None, false, None, false)];
        assert_eq!(
            classify_paired_sample_set(&all, u64::MAX, i64::MIN).0,
            Decision::Inconclusive
        );
        assert_eq!(
            classify_paired_sample_set(&all, u64::MAX, i64::MAX).0,
            Decision::Inconclusive
        );
        assert_eq!(
            compare_paired_endpoint(i128::MIN, u64::MAX, i64::MIN),
            Ordering::Greater
        );
        assert_eq!(
            compare_paired_endpoint(i128::MAX, u64::MAX, i64::MAX),
            Ordering::Less
        );
    }
}

mod gate_tests {
    use vesper_bench::*;

    fn segment(
        lower: Option<i128>,
        lower_inclusive: bool,
        upper: Option<i128>,
        upper_inclusive: bool,
    ) -> SignedRankConfidenceSegment {
        SignedRankConfidenceSegment {
            lower_times_two: lower,
            lower_inclusive,
            upper_times_two: upper,
            upper_inclusive,
        }
    }

    #[test]
    fn threshold_classification_respects_open_closed_disconnected_and_empty_sets() {
        assert_eq!(
            classify_signed_rank_set(&[], 0),
            (
                Decision::Inconclusive,
                Some(SignedRankGateInconclusive::EmptyConfidenceSet)
            )
        );
        let right_open = [segment(Some(0), false, Some(2), true)];
        assert_eq!(
            classify_signed_rank_set(&right_open, 0),
            (Decision::Regression, None)
        );
        let right_closed = [segment(Some(0), true, Some(2), true)];
        assert_eq!(
            classify_signed_rank_set(&right_closed, 0),
            (
                Decision::Inconclusive,
                Some(SignedRankGateInconclusive::ThresholdNotSeparated)
            )
        );
        let left_open = [segment(None, false, Some(0), false)];
        assert_eq!(
            classify_signed_rank_set(&left_open, 0),
            (Decision::Pass, None)
        );
        let disconnected = [
            segment(Some(-4), true, Some(-2), true),
            segment(Some(2), true, Some(4), true),
        ];
        assert_eq!(
            classify_signed_rank_set(&disconnected, 0),
            (
                Decision::Inconclusive,
                Some(SignedRankGateInconclusive::ThresholdNotSeparated)
            )
        );
        let unbounded_right = [segment(Some(2), true, None, false)];
        assert_eq!(
            classify_signed_rank_set(&unbounded_right, 0),
            (Decision::Regression, None)
        );
        let all = [segment(None, false, None, false)];
        assert_eq!(
            classify_signed_rank_set(&all, i64::MIN).0,
            Decision::Inconclusive
        );
        assert_eq!(
            classify_signed_rank_set(&all, i64::MAX).0,
            Decision::Inconclusive
        );
    }
}
