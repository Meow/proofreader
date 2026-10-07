//! Identifier case classification, shared by the naming readers.

/// Whether `name` is lowerCamelCase: after any leading underscores it starts with a lowercase
/// ASCII letter and it contains an uppercase ASCII letter.
///
/// snake_case, SCREAMING_CASE and ConstantStyle names are not lowerCamelCase.
pub fn is_lower_camel_case(name: &str) -> bool {
    let trimmed = name.trim_start_matches('_');
    trimmed.starts_with(|c: char| c.is_ascii_lowercase())
        && trimmed.contains(|c: char| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_names() {
        for camel in ["myVar", "doThing", "_privateThing", "aB"] {
            assert!(is_lower_camel_case(camel), "{camel}");
        }
        for fine in [
            "snake_case",
            "SCREAMING_CASE",
            "ConstantStyle",
            "PlayerSpawn",
            "x",
            "trailing_",
            "_",
            "_G",
            "",
        ] {
            assert!(!is_lower_camel_case(fine), "{fine}");
        }
    }
}
