//! Applies the edits of several fixes to a text, rejecting fixes that conflict.

use std::ops::Range;

use crate::offense::Edit;

/// Whether two edit ranges conflict: they share bytes, or one is an insertion strictly inside the other.
fn conflicts(a: &Range<usize>, b: &Range<usize>) -> bool {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => false,
        (true, false) => b.start < a.start && a.start < b.end,
        (false, true) => a.start < b.start && b.start < a.end,
        (false, false) => a.start < b.end && b.start < a.end,
    }
}

/// Applies `fixes` (pairs of offense index and edits, in priority order) to `text`.
///
/// A fix is accepted only if none of its edits conflicts with an edit already accepted or with
/// another edit of the same fix, and every edit lies on character boundaries within the text.
/// Insertions at the same offset are all kept, in the order they were accepted, before any
/// replacement starting there. Returns the new text and the sorted indexes of the applied fixes.
pub fn apply(text: &str, fixes: Vec<(usize, Vec<Edit>)>) -> (String, Vec<usize>) {
    let mut accepted: Vec<Edit> = Vec::new();
    let mut applied = Vec::new();
    for (index, edits) in fixes {
        let valid = edits.iter().enumerate().all(|(position, edit)| {
            edit.range.start <= edit.range.end
                && edit.range.end <= text.len()
                && text.is_char_boundary(edit.range.start)
                && text.is_char_boundary(edit.range.end)
                && !accepted
                    .iter()
                    .any(|other| conflicts(&edit.range, &other.range))
                && !edits[..position].iter().any(|other| {
                    conflicts(&edit.range, &other.range)
                        || (edit.range == other.range && !edit.range.is_empty())
                })
        });
        if valid && !edits.is_empty() {
            accepted.extend(edits);
            applied.push(index);
        }
    }
    let mut order: Vec<(usize, &Edit)> = accepted.iter().enumerate().collect();
    order.sort_by_key(|(sequence, edit)| (edit.range.start, !edit.range.is_empty(), *sequence));
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for (_, edit) in order {
        output.push_str(&text[cursor..edit.range.start]);
        output.push_str(&edit.replacement);
        cursor = edit.range.end;
    }
    output.push_str(&text[cursor..]);
    applied.sort_unstable();
    (output, applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_non_overlapping_edits() {
        let (text, applied) = apply(
            "local a = 1  \nlocal b",
            vec![
                (0, vec![Edit::remove(11..13)]),
                (1, vec![Edit::replace(20..21, "c")]),
            ],
        );
        assert_eq!(text, "local a = 1\nlocal c");
        assert_eq!(applied, vec![0, 1]);
    }

    #[test]
    fn rejects_overlapping_fix_entirely() {
        let (text, applied) = apply(
            "abcdef",
            vec![
                (0, vec![Edit::replace(1..3, "X")]),
                (1, vec![Edit::insert(0, "<"), Edit::replace(2..4, "Y")]),
            ],
        );
        assert_eq!(text, "aXdef");
        assert_eq!(applied, vec![0]);
    }

    #[test]
    fn insertions_at_the_same_offset_are_kept_in_order() {
        let (text, applied) = apply(
            "ab",
            vec![
                (0, vec![Edit::insert(1, "1")]),
                (1, vec![Edit::insert(1, "2")]),
                (2, vec![Edit::replace(1..2, "B")]),
            ],
        );
        assert_eq!(text, "a12B");
        assert_eq!(applied, vec![0, 1, 2]);
    }

    #[test]
    fn insertion_inside_a_replacement_conflicts() {
        let (text, applied) = apply(
            "abcd",
            vec![
                (0, vec![Edit::replace(0..4, "x")]),
                (1, vec![Edit::insert(2, "!")]),
            ],
        );
        assert_eq!(text, "x");
        assert_eq!(applied, vec![0]);
    }

    #[test]
    fn adjacent_edits_do_not_conflict() {
        let (text, applied) = apply(
            "abcd",
            vec![
                (1, vec![Edit::replace(2..4, "Z")]),
                (0, vec![Edit::replace(0..2, "Y")]),
            ],
        );
        assert_eq!(text, "YZ");
        assert_eq!(applied, vec![0, 1]);
    }

    #[test]
    fn invalid_edits_are_rejected() {
        let (text, applied) = apply(
            "é",
            vec![
                (0, vec![Edit::remove(1..2)]),
                (1, vec![Edit::remove(0..9)]),
                (2, vec![]),
            ],
        );
        assert_eq!(text, "é");
        assert!(applied.is_empty());
    }

    #[test]
    fn self_overlapping_fix_is_rejected() {
        let (text, applied) = apply(
            "abc",
            vec![(0, vec![Edit::remove(0..2), Edit::remove(1..3)])],
        );
        assert_eq!(text, "abc");
        assert!(applied.is_empty());
    }
}
