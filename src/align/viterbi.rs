//! Finding the most likely walk through the slides.

/// Going on to the next slide. A good match is worth 1.
const NEXT_COST: f32 = 0.3;
/// Added for each slide skipped, on top of going to the next one for each, so going
/// through a slide without text is cheaper than skipping it.
const SKIP_COST: f32 = 0.3;
/// Going back by one slide; it's rare.
const BACK_COST: f32 = 1.5;
/// Added for each further slide gone back.
const BACK_STEP_COST: f32 = 0.1;
/// Jumps further than this many slides, either way, all cost `FAR_JUMP_COST`.
const MAX_NEAR_JUMP: usize = 5;
/// No less than any nearer jump, which keeps the Viterbi pass linear in the number
/// of slides.
const FAR_JUMP_COST: f32 = 2.5;

/// What going from slide `from` to slide `to` costs, for jumps of at most
/// `MAX_NEAR_JUMP`.
fn near_jump_cost(from: usize, to: usize) -> f32 {
    if to >= from {
        let steps = (to - from) as f32;
        steps * NEXT_COST + (steps - 1.).max(0.) * SKIP_COST
    } else {
        BACK_COST + (from - to - 1) as f32 * BACK_STEP_COST
    }
}

/// The most likely slide for each slot, given how well each matches (`emissions`, by
/// slot) and that the video starts after slide `start`.
pub(super) fn viterbi(emissions: &[Vec<f32>], slide_count: usize, start: usize) -> Vec<usize> {
    let near =
        |to: usize| to.saturating_sub(MAX_NEAR_JUMP)..(to + MAX_NEAR_JUMP + 1).min(slide_count);
    let jump_cost = |from: usize, to: usize| {
        if from.abs_diff(to) <= MAX_NEAR_JUMP {
            near_jump_cost(from, to)
        } else {
            FAR_JUMP_COST
        }
    };

    let mut score: Vec<f32> = (0..slide_count)
        .map(|slide| emissions[0][slide] - jump_cost(start, slide))
        .collect();
    // For each slot after the first, the slide before it on the best path to each slide.
    let mut previous: Vec<Vec<usize>> = Vec::with_capacity(emissions.len());
    for emission in &emissions[1..] {
        // Every slide can be reached from the best one at `FAR_JUMP_COST`, which is
        // no less than any nearer jump, so only nearer slides have to be checked too.
        let (best, best_score) = score
            .iter()
            .copied()
            .enumerate()
            .fold((0, f32::NEG_INFINITY), |a, b| if b.1 > a.1 { b } else { a });
        let mut next = Vec::with_capacity(slide_count);
        let mut from = Vec::with_capacity(slide_count);
        for to in 0..slide_count {
            let (mut arg, mut max) = (best, best_score - FAR_JUMP_COST);
            for prev in near(to) {
                let candidate = score[prev] - near_jump_cost(prev, to);
                if candidate > max {
                    (arg, max) = (prev, candidate);
                }
            }
            next.push(max + emission[to]);
            from.push(arg);
        }
        score = next;
        previous.push(from);
    }

    let mut slide = score
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(slide, _)| slide);
    let mut path = vec![slide];
    for from in previous.iter().rev() {
        slide = from[slide];
        path.push(slide);
    }
    path.reverse();
    path
}
