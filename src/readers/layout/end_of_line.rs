//! `Layout/EndOfLine`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};

/// Flags carriage returns: files must use LF line endings.
///
/// Reported once per file, at the first carriage return; the fix drops the `\r` of every CRLF
/// pair and turns lone carriage returns (which Lua treats as line breaks) into `\n`.
pub struct EndOfLine;

impl Reader for EndOfLine {
    fn name(&self) -> &'static str {
        "Layout/EndOfLine"
    }

    fn description(&self) -> &'static str {
        "Checks for Windows-style (CRLF) line endings."
    }

    fn investigate(&self, ctx: &mut Context) {
        let text = ctx.source.text.as_str();
        let bytes = text.as_bytes();
        let edits: Vec<Edit> = text
            .match_indices('\r')
            .map(|(index, _)| {
                if bytes.get(index + 1) == Some(&b'\n') {
                    Edit::remove(index..index + 1)
                } else {
                    Edit::replace(index..index + 1, "\n")
                }
            })
            .collect();
        let Some(first) = edits.first().map(|edit| edit.range.clone()) else {
            return;
        };
        ctx.add_offense_with_fix(first, "Carriage return character detected.", edits);
    }
}

inventory::submit! { Registration(|| Box::new(EndOfLine)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/EndOfLine";

    #[test]
    fn accepts_lf() {
        expect_no_offenses(READER, "a()\nb()\n");
        expect_no_offenses(READER, "");
    }

    #[test]
    fn flags_the_first_carriage_return_once() {
        expect_offense(
            READER,
            "a()\r\nb()\r\n",
            1,
            4,
            "Carriage return character detected.",
        );
        expect_offense(
            READER,
            "a()\nb()\r\n",
            2,
            4,
            "Carriage return character detected.",
        );
        expect_offense(
            READER,
            "a()\rb()\n",
            1,
            4,
            "Carriage return character detected.",
        );
    }

    #[test]
    fn autocorrects() {
        expect_correction(READER, "a()\r\nb()\r\n", "a()\nb()\n");
        expect_correction(READER, "s = [[x\r\ny]]\r\n", "s = [[x\ny]]\n");
        expect_correction(READER, "a()\rb()\r\n\r\n", "a()\nb()\n\n");
        expect_correction(READER, "a()\r\r\n", "a()\n\n");
    }
}
