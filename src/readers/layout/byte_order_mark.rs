//! `Layout/ByteOrderMark`.

use crate::offense::Edit;
use crate::reader::{Context, Reader, Registration};

/// The UTF-8 encoding of U+FEFF.
const BOM: &str = "\u{feff}";

/// Flags a UTF-8 byte order mark at the start of the file.
pub struct ByteOrderMark;

impl Reader for ByteOrderMark {
    fn name(&self) -> &'static str {
        "Layout/ByteOrderMark"
    }

    fn description(&self) -> &'static str {
        "Checks for a UTF-8 byte order mark at the start of the file."
    }

    fn investigate(&self, ctx: &mut Context) {
        if ctx.source.text.starts_with(BOM) {
            let range = 0..BOM.len();
            ctx.add_offense_with_fix(
                range.clone(),
                "Byte order mark detected.",
                vec![Edit::remove(range)],
            );
        }
    }
}

inventory::submit! { Registration(|| Box::new(ByteOrderMark)) }

#[cfg(test)]
mod tests {
    use crate::testing::*;

    const READER: &str = "Layout/ByteOrderMark";

    #[test]
    fn flags_a_leading_bom() {
        expect_offense(READER, "\u{feff}x()\n", 1, 1, "Byte order mark detected.");
        expect_offense(READER, "\u{feff}", 1, 1, "Byte order mark detected.");
        expect_correction(READER, "\u{feff}-- c\nx()\n", "-- c\nx()\n");
    }

    #[test]
    fn ignores_other_positions() {
        expect_no_offenses(READER, "x()\n");
        expect_no_offenses(READER, "s = '\u{feff}'\n");
        expect_no_offenses(READER, "");
    }
}
