# Tuning the alignment

Notes from tuning the constants in `index.rs`, `mod.rs` and `viterbi.rs` against
a real lecture, so the next attempt can start from them rather than from scratch.

## Data

One project: the first three lectures of *Mobile Computing* (Uni Stuttgart, summer
term 2026), with all 21 PDFs of the course (542 slides) and three videos of about
90 minutes each, transcribed with Whisper (about 1,400 segments each). The lectures
only get through chapters 0–2 (slides 0–67), so the other 470 slides act as
distractors, which is realistic: a project holds the whole course.

The project file isn't in the repository. It was at
`C:\Users\Leon\Desktop\slidelytest\Untitled2.slidely`. `Untitled3.slidely` next to
it has the same PDFs and transcripts (plus leftover `embeddings`), so it's not a
second data set.

Everything is in English. Nothing has been tuned on German lectures.

## Ground truth

There was none, so it was made by reading the transcripts next to the slide text
and marking where the lecturer moves on, by cues like "when John arrives at the
university" (the first bullet of ch01 p5) or "here's the spectrum we already
discussed". It's a reading of the transcript, not checked against the video, so
boundaries can be off by a few seconds, and a few stretches accept more than one
slide where the talk fits either.

Not scored: stretches no slide in the project fits. In lecture 1 that's Lukas's
lab introduction (25:31–46:13, his slides aren't in the project); in lecture 2 the
lab announcement at the start (0:00–4:13); in lecture 3 a video demo of a
software-defined radio (70:53–85:02).

Format: video (1-based), start, end, the slides (global 0-based index across the
PDFs, in project order) that count as right.

```text
# v1: organization (ch00 = slides 0-7), then ch01 (slides 8-40)
1 0:00 2:31 0
1 2:31 3:58 1
1 3:58 7:52 2
1 7:52 10:34 3
1 10:34 20:20 4
1 20:20 23:40 5
1 23:40 24:57 6
1 24:57 25:31 6,7
1 46:13 47:22 8
1 47:22 49:17 9
1 49:17 51:53 10
1 51:53 72:59 11
1 72:59 76:37 12
1 76:37 81:54 13
1 81:54 84:30 14
1 84:30 90:00 15
# v2
2 4:13 7:04 15
2 7:04 14:42 16
2 14:42 20:12 17
2 20:12 20:39 18,19
2 20:39 25:29 19
2 25:29 29:17 20
2 29:17 33:28 21
2 33:28 37:06 22
2 37:06 41:04 23
2 41:04 42:21 24
2 42:21 42:51 25,26
2 42:51 48:50 27
2 48:50 60:29 28
2 60:29 63:55 29
2 63:55 64:49 30
2 64:49 67:38 31
2 67:38 67:53 32
2 67:53 70:15 33
2 70:15 70:59 34
2 70:59 71:55 35
2 71:55 74:29 36
2 74:29 75:26 37
2 75:26 76:36 38
2 76:36 77:00 39
2 77:00 78:18 39,40,41
2 78:18 78:30 41,42
2 78:30 84:55 42
2 84:55 86:23 43
2 86:23 86:51 44
2 86:51 88:26 45
2 88:26 91:00 46
# v3: ch02 part 1 (slides 41-64), then part 2 (65-81)
3 0:00 1:52 44,45,46
3 1:52 5:54 46
3 5:54 6:56 47
3 6:56 10:55 48
3 10:55 13:54 49
3 13:54 22:34 50
3 22:34 27:49 51
3 27:49 28:42 52
3 28:42 31:17 53
3 31:17 31:34 53,54,55
3 31:34 38:35 55
3 38:35 41:07 56
3 41:07 41:35 57
3 41:35 53:19 57,58
3 53:19 54:08 58
3 54:08 57:16 59
3 57:16 62:21 60
3 62:21 64:17 62
3 64:17 66:23 61
3 66:23 70:20 62
3 70:20 70:53 63
3 85:02 86:04 65,66
3 86:04 90:00 66,67
```

The lecture is a hard case in places: the lecturer often talks for minutes about
topics of later chapters (data consistency, Mobile IP, link-state routing) while a
scenario slide is shown, and slides 9, 18 and 26 are the same "Overview" slide.

## Harness

A scratch crate with a copy of `src/align` in which every constant was replaced by
a field of a `Params` struct (read from a thread-local, so candidates can run in
parallel), with stand-ins for `PageText`, `TextLine`, `title_index` and `Segment`
and serde types for the project file. It also included the real `src/align/mod.rs`
through `#[path = ...]`, to check that the code in the repository gives exactly the
alignment of the tuned copy. It was in a temp folder; rebuilding it takes an hour
or so. One run over the three videos takes about 0.3 s optimized (1.6 s in the
`print_alignment` test, which is a debug build).

Metrics, over the labelled seconds:

- **acc**: the fraction on a slide that counts as right (`Alignment::slide_at`).
- **near**: on a right slide or one next to it.
- **far**: more than 5 slides off, i.e. the walk went somewhere else entirely.
- **starts**: of the 57 slides labelled alone somewhere, how many have their longest
  span (what `Project::slide_start` seeks to) starting inside, or up to 15 s before,
  an interval where they're right.

Objective: `acc + 0.3 * starts / 57`.

Search, in the order it was done:

1. Coordinate descent over a grid of values per constant.
2. Random local search: change 1–3 constants to random grid values, 32 candidates
   at a time, keep the best. It found nothing beyond step 1.
3. A sensitivity table: the score for every grid value of each constant, the others
   fixed.
4. Leave one video out: tune on two lectures, score the third.
5. Search on a smoothed objective, the mean over a configuration and all its
   neighbours one grid step away, to avoid peaks that only fit this lecture.
6. Ablations of the final configuration.

Since one boundary placed a few minutes off moves acc by 3–4 points, the score is
jumpy, and single best configurations overfit. Prefer values in the middle of a
flat region, and check on held-out videos.

## Results

| | acc | near | far | starts |
| --- | --- | --- | --- | --- |
| before | 58.6% | 74.1% | 18.3% | 27/57 |
| after | 85.1% | 99.7% | 0% | 39/57 |

Per video, acc went from 67% to 85% (v1), 66% to 80% (v2) and 43% to 91% (v3).

Leave one video out, with short words as whole terms fixed and everything else
tuned on the other two: 84% / 72% / 88%, never far off. (Before whole-word terms,
when the search dropped short words instead, it was 62% / 72% / 78%.)

### What changed, and why

| Constant | Before | After |
| --- | --- | --- |
| `WHOLE_WORD_LEN` (new) | – | 5 |
| `EMISSION_POWER` (new) | (1) | 3 |
| `MEAN_EMISSION_WEIGHT` (new) | (0) | 0.5 |
| `GOOD_MATCH_QUANTILE` | 0.9 | 0.995 |
| `MAX_EMISSION` | 2 | 1 |
| `BLANK_SLIDE_EMISSION` | 0.9 | 0.7 |
| `TITLE_WEIGHT` | 2 | 1.5 |
| `SLOT_CONTEXT` | 10 s | 5 s |
| `CONTEXT_WEIGHT` | 0.5 | 0.35 |

Ablations, each removed from the final configuration on its own:

| Removed | acc | far |
| --- | --- | --- |
| nothing | 83.7–85.1% | 0% |
| short words as whole terms | 75.3% | 0% |
| mean emission subtraction | 79.8% | 2.7% |
| emission power (back to 1) | 71.6% | 8.8% |

- **The emission power** is what stopped the far jumps. With 542 slides, the best
  slide of a slot is usually an unrelated one that matches a little by chance
  (0.5–0.8 after normalizing), while the right slide often only scores 0.2–0.5.
  Summed over minutes, that noise outweighed any jump cost. Cubing scores below the
  good match makes them negligible. It only works together with a good match close
  to the best (`GOOD_MATCH_QUANTILE` 0.995) and the clamp at 1; at 0.9 the power
  does much less.
- **Short words as whole terms.** All trigrams of a 3–4 letter word are at its
  start or end (" gp", "gps", "ps "), and those are shared by many longer words, so
  short words matched noise. Dropping words under 5 letters helped about as much,
  but whole terms keep acronyms (GPS, LTE, TCP, BLE), which matter on technical
  slides, and generalized much better in the leave-one-out runs.
- **Mean emission subtraction** stops "hub" slides: ch02 p6 (*Line of Sight
  Communication*) has words that come up all the time, and before, it won for
  11 minutes of v3 where slides 57/58 were shown.
- **Smaller context** sharpens boundaries. `SLOT_CONTEXT` 0–10 s are all about as
  good; 15 s and more is worse.
- **`TITLE_WEIGHT`** 1–1.5 is best, 2 and more is 5 points worse. Kept at 1.5
  rather than dropping title weighting, in case other slide decks rely on it more.

### Sensitivity at the final point

Rough acc for other values, the rest fixed (from the sensitivity runs; the final
point differs a little from the one these were measured at, so read them as
shapes, not exact numbers):

- `EMISSION_POWER`: 1 → 0.72, 2 → 0.80, 3 → 0.85, 4 → 0.79, 6 → 0.75. A clear peak.
- `MEAN_EMISSION_WEIGHT`: 0 → 0.80, 0.25 → 0.83, 0.5 → 0.85, 1 → 0.79.
- `GOOD_MATCH_QUANTILE`: 0.7 → 0.57, 0.9 → 0.71, 0.98 → 0.82, 0.995–1 → 0.85.
- `NEXT_COST`: 0.1–0.3 flat, 0.5 → 0.82, 0.75 → 0.75, 1 → 0.61. Watch out: high
  costs relative to the (now smaller) emissions stop the walk from moving on.
- `SLOT`: 5 s → 0.74, 10–15 s → 0.85, 20 s+ → 0.82. Kept at 10 s for precise starts.
  Changing `SLOT` also changes how much emission there is per second relative to the
  jump costs, so re-tune the costs with it.
- `BLANK_SLIDE_EMISSION`: 0.5–0.7 flat, 0.8+ worse.
- `HEADER_FRACTION`: 0.3–0.5 flat, 0.65+ worse.

Flat on this data, left as they were: `REVISIT_COST`, `RESUME_SLIDES`, `SKIP_COST`,
`BACK_STEP_COST`, `MAX_NEAR_JUMP`, `FAR_JUMP_COST` (from 2.5 up), `BACK_COST` (from
1 up). They only matter for things this lecture barely does (going back, skipping,
a later video revisiting slides), so this data says nothing about them; the unit
tests in `mod.rs` cover their behaviour.

## Tried and not adopted

- **Far jumps that cost more with distance**, forward and backward separately
  (`min(base + per_slide * d, cap)`, which stays linear in the number of slides
  with prefix and suffix maxima). Did nothing once the emission power was in; the
  emissions, not the transitions, were the problem.
- **Subtracting the mean over slides from each slot.** Worse (−3 points).
- **"Speech IDF"**: weighting terms down by how many transcript segments they're
  in, to handle filler words in any language. Small gain (about +1 point and a
  start or two at power 0.25–0.5), not worth the code. It didn't replace the
  whole-word terms either.
- **A scale on all emissions**, i.e. all transition costs together: best at 0.5–1,
  so the costs were left as they were.

## What's left

The remaining errors are boundaries one slide early or late, mostly where the
lecturer talks ahead about the next slide's topic, e.g. v1 moves from slide 11 to
12 nine minutes early, during talk about Mobile IP, which slide 12's labels mention;
v2 moves from 27 to 28 five minutes early. Bag-of-words matching can't tell these
apart; it would take something that knows what's on screen.

The walk also goes back less now: it misses the one real detour (v3 slide 62 → 61
→ 62). `goes_back_when_it_lasts` still passes.

## Next time

- Tune on more than one course, ideally a German one and one with fewer slides per
  lecture, and check the values above hold. The biggest risk of overfitting is in
  the sharp peaks: `EMISSION_POWER` and `MEAN_EMISSION_WEIGHT`.
- Label ground truth from the video where possible; a transcript-only reading is
  ambiguous around boundaries.
- Score per second and on longest-span starts together; either alone hides a lot.
- Look at the per-slot emissions (best slides next to the right one) before
  tuning; that's what showed the noise floor and the hub slide here.
