//! Changing a text in one pass.

/// `src` with the byte ranges `(start, end, replacement)` of `edits`
/// replaced. Edits are taken in order of position, and one that overlaps
/// an earlier one is left out, so the result does not depend on the order
/// they come in. Returns the new text and how many edits were made.
pub(crate) fn apply_edits<S: AsRef<str>>(
    src: &str,
    edits: impl IntoIterator<Item = (usize, usize, S)>,
) -> (String, usize) {
    let mut edits: Vec<(usize, usize, S)> = edits.into_iter().collect();
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    let mut made = 0;
    for (start, end, text) in edits {
        if start < at || start > end || end > src.len() {
            continue;
        }
        out.push_str(&src[at..start]);
        out.push_str(text.as_ref());
        at = end;
        made += 1;
    }
    out.push_str(&src[at..]);
    (out, made)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_in_any_order() {
        let edits = [(6, 11, "there"), (0, 0, ">> "), (11, 11, "!")];
        assert_eq!(apply_edits("hello world", edits), (">> hello there!".to_string(), 3));
        // an edit overlapping an earlier one is left out
        assert_eq!(
            apply_edits("abcdef", [(1, 4, "X"), (2, 3, "Y"), (5, 6, "Z")]),
            ("aXeZ".to_string(), 2)
        );
        assert_eq!(apply_edits("abc", [(2, 9, "x"), (2, 1, "y")]), ("abc".to_string(), 0));
    }
}
