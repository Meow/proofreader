//! `Layout/EndOfLine`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};

/// Flags carriage returns: files must use LF line endings.
///
/// Reported once per file, at the first carriage return; the fix drops the `\r` of every CRLF
/// pair and turns lone carriage returns (which Lua treats as line breaks) into `\n`. Carriage
/// returns inside Lumen markup tokens (quoted attribute values and `<!-- -->` comments spanning
/// lines) are reported but never changed.
pub struct EndOfLine;

impl Reader for EndOfLine {
    fn name(&self) -> &'static str {
        "Layout/EndOfLine"
    }

    fn description(&self) -> &'static str {
        "Checks for Windows-style (CRLF) line endings."
    }

    fn investigate(&self, ctx: &mut Context) {
        let source = ctx.source;
        let text = source.text.as_str();
        let bytes = text.as_bytes();
        let Some(first) = text.find('\r') else {
            return;
        };
        let edits: Vec<Edit> = text
            .match_indices('\r')
            .filter(|&(index, _)| !source.in_markup(index))
            .map(|(index, _)| {
                if bytes.get(index + 1) == Some(&b'\n') {
                    Edit::remove(index..index + 1)
                } else {
                    Edit::replace(index..index + 1, "\n")
                }
            })
            .collect();
        let offense = ctx.add_offense(first..first + 1, "Carriage return character detected.");
        if !edits.is_empty() {
            offense.with_fix(edits);
        }
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

    #[test]
    fn never_changes_carriage_returns_inside_markup() {
        let src = "return <view title=\"a\r\nb\">\r\n</view>\r\n";
        expect_offense_lumen(READER, src, 1, 22, "Carriage return character detected.");
        expect_correction_lumen(READER, src, "return <view title=\"a\r\nb\">\n</view>\n");
        let markup_only = "return <view title=\"a\r\nb\"/>\n";
        expect_offense_lumen(
            READER,
            markup_only,
            1,
            22,
            "Carriage return character detected.",
        );
        assert_eq!(autocorrect_lumen(READER, markup_only), markup_only);
    }
}
