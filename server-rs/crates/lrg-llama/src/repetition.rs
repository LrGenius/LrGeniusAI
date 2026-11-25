//! Stopping an answer that has started to repeat itself.
//!
//! A small model decoding at a low temperature can fall into a loop: the same
//! few tokens, or the same sentence, over and over until Max Tokens runs out.
//! Issue #490 showed it on 10-20 % of photos with Gemma 4 12B: about 100 s per
//! photo spent decoding a single JSON string to the token limit, then a parse
//! error that told the user to raise Max Tokens, which only made the next run
//! slower. Spotting the loop as it forms ends the photo in a fraction of that
//! time, with an error that says what happened.
//!
//! What counts as a loop is deliberately narrow: a run of tokens that repeats
//! *exactly*, at least [`MIN_REPEATS`] times in a row, covering at least
//! [`MIN_LOOP_TOKENS`] tokens. Prose and keyword lists do not repeat that way;
//! degenerate decoding does. A loop that varies (a counter, a drifting phrase)
//! is not caught here — the schema's length bounds and Max Tokens end those.

use std::collections::VecDeque;

/// The longest repeated unit looked for, in tokens: a sentence or two.
pub(crate) const MAX_PERIOD: usize = 64;

/// How many times in a row the unit has to appear. Twice can be emphasis or a
/// legitimately repeated phrase; three identical runs back to back are not.
pub(crate) const MIN_REPEATS: usize = 3;

/// The fewest tokens a loop has to cover before it counts, so that a short
/// unit ("ha ha ha", a run of the same punctuation) is not called a loop
/// after a handful of tokens.
pub(crate) const MIN_LOOP_TOKENS: usize = 48;

/// Watches an answer token by token and says when it has started to loop.
///
/// For every period `p` up to [`MAX_PERIOD`] it counts how many tokens in a
/// row have equalled the token `p` places before them. A unit of `p` tokens
/// repeated `r` times makes that count `(r - 1) * p`, so the check is two
/// comparisons per period per token — nothing that shows up beside a decode
/// step.
#[derive(Debug, Clone)]
pub(crate) struct RepetitionGuard {
    /// The last [`MAX_PERIOD`] tokens, oldest first.
    recent: VecDeque<i32>,
    /// `runs[p]`: consecutive tokens equal to the one `p` places back.
    runs: [usize; MAX_PERIOD + 1],
}

impl Default for RepetitionGuard {
    fn default() -> Self {
        RepetitionGuard {
            recent: VecDeque::with_capacity(MAX_PERIOD),
            runs: [0; MAX_PERIOD + 1],
        }
    }
}

impl RepetitionGuard {
    /// Take the next token of the answer. Returns the length of the repeated
    /// unit once the answer is looping, `None` until then.
    pub(crate) fn push(&mut self, token: i32) -> Option<usize> {
        let mut looping = None;
        let len = self.recent.len();
        for period in 1..=len {
            if self.recent[len - period] == token {
                self.runs[period] += 1;
            } else {
                self.runs[period] = 0;
            }
            let run = self.runs[period];
            // The repetition covers the run plus the first copy of the unit.
            if looping.is_none()
                && run >= (MIN_REPEATS - 1) * period
                && run + period >= MIN_LOOP_TOKENS
            {
                looping = Some(period);
            }
        }
        if self.recent.len() == MAX_PERIOD {
            self.recent.pop_front();
        }
        self.recent.push_back(token);
        looping
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `tokens` and returns the index of the token at which the guard
    /// fired, with the period it reported.
    fn first_alarm(tokens: impl IntoIterator<Item = i32>) -> Option<(usize, usize)> {
        let mut guard = RepetitionGuard::default();
        tokens
            .into_iter()
            .enumerate()
            .find_map(|(i, t)| guard.push(t).map(|p| (i, p)))
    }

    /// Distinct tokens: what a normal answer looks like to the guard.
    fn prose(n: usize) -> Vec<i32> {
        (0..n).map(|i| 1000 + i32::try_from(i).unwrap()).collect()
    }

    #[test]
    fn a_normal_answer_is_left_alone() {
        assert_eq!(first_alarm(prose(4000)), None);
    }

    #[test]
    fn the_same_token_over_and_over_is_a_loop() {
        // 1.35 characters per token, as in the issue's log: a run of one
        // short token.
        let mut tokens = prose(20);
        tokens.extend(std::iter::repeat_n(7, 200));
        let (at, period) = first_alarm(tokens).expect("should stop");
        assert_eq!(period, 1);
        // It fires once the run spans MIN_LOOP_TOKENS, not at Max Tokens.
        assert_eq!(at, 20 + MIN_LOOP_TOKENS - 1);
    }

    #[test]
    fn a_repeated_sentence_is_a_loop_after_three_copies() {
        let sentence: Vec<i32> = (0..20).map(|i| 500 + i).collect();
        let mut tokens = prose(30);
        for _ in 0..10 {
            tokens.extend(&sentence);
        }
        let (at, period) = first_alarm(tokens).expect("should stop");
        assert_eq!(period, 20);
        // Third copy complete: 30 + 3 * 20 tokens, the last at index 89.
        assert_eq!(at, 30 + MIN_REPEATS * 20 - 1);
    }

    #[test]
    fn a_phrase_said_twice_is_not_a_loop() {
        let sentence: Vec<i32> = (0..30).map(|i| 500 + i).collect();
        let mut tokens = prose(10);
        tokens.extend(&sentence);
        tokens.extend(&sentence);
        tokens.extend(prose(100).iter().map(|t| t + 5000));
        assert_eq!(first_alarm(tokens), None);
    }

    #[test]
    fn a_short_repetition_needs_the_minimum_span() {
        // "ha ha ha" a few times is not yet a loop...
        let mut tokens = prose(10);
        tokens.extend([1, 2].repeat(10));
        tokens.extend(prose(50).iter().map(|t| t + 5000));
        assert_eq!(first_alarm(tokens), None);
        // ...but past MIN_LOOP_TOKENS it is.
        let mut tokens = prose(10);
        tokens.extend([1, 2].repeat(40));
        let (_, period) = first_alarm(tokens).expect("should stop");
        assert_eq!(period, 2);
    }

    #[test]
    fn a_unit_longer_than_the_window_is_not_reported() {
        let unit: Vec<i32> = (0..i32::try_from(MAX_PERIOD).unwrap() + 10)
            .map(|i| 500 + i)
            .collect();
        let mut tokens = Vec::new();
        for _ in 0..5 {
            tokens.extend(&unit);
        }
        assert_eq!(first_alarm(tokens), None);
    }

    /// A JSON keyword list repeats its punctuation between every item; that
    /// alone must never look like a loop.
    #[test]
    fn json_structure_between_distinct_items_is_not_a_loop() {
        // `"`, word, `",` per keyword, each word distinct.
        let mut tokens = vec![1, 2];
        for word in 0..200 {
            tokens.extend([3, 10_000 + word, 4]);
        }
        assert_eq!(first_alarm(tokens), None);
    }

    /// The same keyword emitted again and again is the list form of a loop.
    #[test]
    fn the_same_list_item_over_and_over_is_a_loop() {
        let mut tokens = vec![1, 2];
        for word in 0..5 {
            tokens.extend([3, 10_000 + word, 4]);
        }
        for _ in 0..40 {
            tokens.extend([3, 777, 4]);
        }
        let (_, period) = first_alarm(tokens).expect("should stop");
        assert_eq!(period, 3);
    }
}
