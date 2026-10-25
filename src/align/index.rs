//! Comparing what's said with the text of the slides.

use std::collections::{HashMap, HashSet};

use super::Slot;
use crate::pdf::{PageText, TextLine, title_index};
use crate::video::Segment;

/// Lines found on more than this fraction of the pages, numbers aside, are taken to
/// be headers or footers and ignored.
const HEADER_FRACTION: f32 = 0.5;
/// Headers are only looked for in PDFs with at least this many pages; in shorter
/// ones, repeated lines are as likely to be content.
const HEADER_MIN_PAGES: usize = 4;
/// How many times the words of a slide's title count; lecturers tend to say it.
const TITLE_WEIGHT: f32 = 1.5;
/// Words shorter than this, in characters, are ignored.
const MIN_WORD_LEN: usize = 3;
/// Words shorter than this, in characters, are compared as a whole rather than by
/// their trigrams: all of a short word's trigrams are at its start or end, which many
/// other words share, so "gps" would match "group" and "steps".
const WHOLE_WORD_LEN: usize = 5;
/// Similarity at most this many times what a good match scores, so one window
/// doesn't outweigh many.
const MAX_EMISSION: f32 = 1.;
/// Similarities are raised to this power, so a slot that matches one slide well
/// counts for more than one that matches many a little. With hundreds of slides, some
/// slide always matches a little by chance, and enough of that would otherwise draw
/// the walk away, e.g. to a later chapter the lecturer mentions in passing.
const EMISSION_POWER: f32 = 3.;
/// How much of each slide's mean emission over the video is taken off it, so slides
/// with words that come up all the time don't win wherever nothing matches well.
const MEAN_EMISSION_WEIGHT: f32 = 0.5;
/// How a slide without text matches a slot, relative to the better of the slides
/// with text around it. Less, so the walk passes through it where the lecturer goes
/// from one neighbour to the other, rather than staying on it.
const BLANK_SLIDE_EMISSION: f32 = 0.7;
/// Which of the windows' best similarities is taken as a good match, as a fraction
/// of them sorted ascending. Close to the best, so only clear matches count fully.
const GOOD_MATCH_QUANTILE: f32 = 0.995;

/// Words too common to tell slides apart.
const ENGLISH_STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "her", "was", "one",
    "our", "out", "has", "have", "had", "this", "that", "with", "from", "they", "will", "would",
    "there", "their", "what", "which", "when", "then", "than", "them", "these", "those", "into",
    "also", "just", "here", "its", "it's", "your", "about", "some", "very", "now", "okay", "yeah",
    "well", "so", "like", "how", "who", "get", "got", "let", "see", "say", "said", "were", "been",
    "being", "does", "did", "doing",
];
const GERMAN_STOPWORDS: &[&str] = &[
    "der", "die", "das", "den", "dem", "des", "ein", "eine", "einen", "einem", "einer", "eines",
    "und", "oder", "aber", "ist", "sind", "war", "waren", "wird", "werden", "hat", "haben",
    "hatte", "mit", "von", "für", "auf", "aus", "bei", "nach", "über", "unter", "vor", "zum",
    "zur", "auch", "noch", "nur", "schon", "dann", "wenn", "weil", "dass", "als", "wie", "wir",
    "ihr", "sie", "ich", "man", "sich", "nicht", "kein", "keine", "hier", "jetzt", "also", "genau",
    "mal", "halt", "eben", "doch", "ja", "nein", "gut", "okay", "diese", "dieser", "dieses",
    "diesen", "diesem", "was", "wer", "denn", "sehr", "kann", "können", "muss", "müssen", "soll",
    "sollen", "gibt",
];

/// A sparse vector of term weights.
type TermVector = Vec<(u32, f32)>;

/// What the slides and the speech are compared by.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Term {
    /// A character trigram of a word of at least `WHOLE_WORD_LEN`.
    Trigram([char; 3]),
    /// A shorter word, padded with `'\0'`.
    Word([char; WHOLE_WORD_LEN - 1]),
}

/// The slides as TF-IDF vectors over the terms of their words.
pub(super) struct SlideIndex {
    /// Terms found on the slides, by id.
    terms: HashMap<Term, u32>,
    /// Inverse document frequency of each term, by id.
    idf: Vec<f32>,
    /// One per slide, normalized to length 1; empty if it has no text.
    vectors: Vec<TermVector>,
    /// By term id, the slides it's on and its weight there, so a slot is only
    /// compared with the slides it shares terms with.
    postings: Vec<Vec<(u32, f32)>>,
}

impl SlideIndex {
    pub(super) fn new(pdfs: &[Vec<Option<PageText>>]) -> Self {
        let slide_count: usize = pdfs.iter().map(Vec::len).sum();
        let project_headers = headers(pdfs.iter().flatten());
        let mut terms = HashMap::new();
        let mut counts: Vec<HashMap<u32, f32>> = Vec::with_capacity(slide_count);
        for pages in pdfs {
            let pdf_headers = headers(pages);
            for page in pages {
                let lines = page.as_ref().map_or(&[][..], |page| &page.lines[..]);
                let lines: Vec<&TextLine> = lines
                    .iter()
                    .filter(|line| {
                        let key = line_key(&line.text);
                        !project_headers.contains(&key) && !pdf_headers.contains(&key)
                    })
                    .collect();
                // Now that headers are left out.
                let title = title_index(lines.iter().copied());
                let mut slide_counts = HashMap::new();
                for (ix, line) in lines.iter().enumerate() {
                    let weight = if Some(ix) == title { TITLE_WEIGHT } else { 1. };
                    for term in terms_of(&line.text) {
                        let next_id = terms.len() as u32;
                        let id = *terms.entry(term).or_insert(next_id);
                        *slide_counts.entry(id).or_insert(0.) += weight;
                    }
                }
                counts.push(slide_counts);
            }
        }

        let mut document_frequency = vec![0usize; terms.len()];
        for slide_counts in &counts {
            for &id in slide_counts.keys() {
                document_frequency[id as usize] += 1;
            }
        }
        // Terms on every slide weigh next to nothing.
        let idf = document_frequency
            .iter()
            .map(|&df| ((slide_count as f32 + 1.) / df as f32).ln())
            .collect();

        let mut index = Self {
            postings: vec![Vec::new(); terms.len()],
            terms,
            idf,
            vectors: Vec::new(),
        };
        index.vectors = counts
            .into_iter()
            .map(|slide_counts| index.vector(slide_counts))
            .collect();
        for (slide, vector) in index.vectors.iter().enumerate() {
            for &(id, weight) in vector {
                index.postings[id as usize].push((slide as u32, weight));
            }
        }
        index
    }

    /// TF-IDF weights for term counts, normalized to length 1.
    fn vector(&self, counts: HashMap<u32, f32>) -> TermVector {
        let mut vector: TermVector = counts
            .into_iter()
            .map(|(id, count)| (id, (1. + count.ln()) * self.idf[id as usize]))
            .filter(|&(_, weight)| weight > 0.)
            .collect();
        let norm = vector.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();
        if norm > 0. {
            vector.iter_mut().for_each(|(_, w)| *w /= norm);
        }
        vector
    }

    /// How well each slot matches each slide, by slot, then slide; a good match is
    /// about 1.
    pub(super) fn emissions(&self, segments: &[Segment], slots: &[Slot]) -> Vec<Vec<f32>> {
        // Each segment is in several slots, so its terms are counted once up front.
        // Words not on any slide don't tell them apart, so they're left out.
        let segment_counts: Vec<HashMap<u32, f32>> = segments
            .iter()
            .map(|segment| {
                let mut counts = HashMap::new();
                for id in terms_of(&segment.text).filter_map(|term| self.terms.get(&term)) {
                    *counts.entry(*id).or_insert(0.) += 1.;
                }
                counts
            })
            .collect();
        let mut similarities: Vec<Vec<f32>> = slots
            .iter()
            .map(|slot| {
                let mut counts = HashMap::new();
                for &(segment, weight) in &slot.segments {
                    for (&id, &count) in &segment_counts[segment] {
                        *counts.entry(id).or_insert(0.) += count * weight;
                    }
                }
                let mut similarity = vec![0.; self.vectors.len()];
                for (id, weight) in self.vector(counts) {
                    for &(slide, slide_weight) in &self.postings[id as usize] {
                        similarity[slide as usize] += weight * slide_weight;
                    }
                }
                similarity
            })
            .collect();

        let mut best: Vec<f32> = similarities
            .iter()
            .map(|slot| slot.iter().copied().fold(0., f32::max))
            .filter(|&best| best > 0.)
            .collect();
        if best.is_empty() {
            return similarities;
        }
        best.sort_unstable_by(f32::total_cmp);
        let good_match = best[((best.len() - 1) as f32 * GOOD_MATCH_QUANTILE) as usize];
        for similarity in similarities.iter_mut().flatten() {
            *similarity = (*similarity / good_match)
                .min(MAX_EMISSION)
                .powf(EMISSION_POWER);
        }
        let mut means = vec![0.; self.vectors.len()];
        for slot in &similarities {
            for (mean, similarity) in means.iter_mut().zip(slot) {
                *mean += similarity / similarities.len() as f32;
            }
        }
        for slot in &mut similarities {
            for (similarity, mean) in slot.iter_mut().zip(&means) {
                *similarity -= MEAN_EMISSION_WEIGHT * mean;
            }
        }

        // Slides without text get theirs from the nearest ones with text.
        let has_text: Vec<bool> = self.vectors.iter().map(|v| !v.is_empty()).collect();
        let before = |slide: usize| (0..slide).rev().find(|&ix| has_text[ix]);
        let after = |slide: usize| (slide + 1..has_text.len()).find(|&ix| has_text[ix]);
        let blanks: Vec<_> = (0..has_text.len())
            .filter(|&slide| !has_text[slide])
            .map(|slide| (slide, before(slide), after(slide)))
            .collect();
        for slot in &mut similarities {
            for &(slide, before, after) in &blanks {
                let neighbour = before.into_iter().chain(after).map(|ix| slot[ix]);
                slot[slide] = neighbour.fold(0., f32::max) * BLANK_SLIDE_EMISSION;
            }
        }
        similarities
    }
}

/// The lines that are on so many pages that they must be headers or footers, as
/// `line_key`s.
fn headers<'a>(slides: impl IntoIterator<Item = &'a Option<PageText>>) -> HashSet<String> {
    let slides: Vec<_> = slides.into_iter().collect();
    if slides.len() < HEADER_MIN_PAGES {
        return HashSet::new();
    }
    let mut pages_with_line: HashMap<String, usize> = HashMap::new();
    for slide in slides.iter().copied().flatten() {
        let mut keys: Vec<_> = slide
            .lines
            .iter()
            .map(|line| line_key(&line.text))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        for key in keys {
            *pages_with_line.entry(key).or_insert(0) += 1;
        }
    }
    let threshold = slides.len() as f32 * HEADER_FRACTION;
    pages_with_line
        .into_iter()
        .filter(|(key, count)| !key.is_empty() && *count as f32 > threshold)
        .map(|(key, _)| key)
        .collect()
}

/// A line with its numbers left out, so "3 / 40" and "4 / 40" are the same footer.
fn line_key(line: &str) -> String {
    line.chars()
        .filter(|c| !c.is_numeric() && !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The terms of the words in `text`: the character trigrams of each word, padded with
/// a space on both sides so its start and end are trigrams too, or short words as a
/// whole. Trigrams rather than whole words match inflected forms, compounds, and words
/// the transcription misspelled.
fn terms_of(text: &str) -> impl Iterator<Item = Term> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty() && !word.chars().all(char::is_numeric))
        .map(str::to_lowercase)
        .filter(|word| {
            word.chars().count() >= MIN_WORD_LEN
                && !ENGLISH_STOPWORDS.contains(&word.as_str())
                && !GERMAN_STOPWORDS.contains(&word.as_str())
        })
        .flat_map(|word| {
            if word.chars().count() < WHOLE_WORD_LEN {
                let mut chars = ['\0'; WHOLE_WORD_LEN - 1];
                chars.iter_mut().zip(word.chars()).for_each(|(c, w)| *c = w);
                return vec![Term::Word(chars)];
            }
            let chars: Vec<char> = std::iter::once(' ')
                .chain(word.chars())
                .chain(std::iter::once(' '))
                .collect();
            chars
                .windows(3)
                .map(|w| Term::Trigram([w[0], w[1], w[2]]))
                .collect()
        })
}
