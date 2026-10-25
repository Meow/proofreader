//! Identifier case classification and the naming styles of the naming readers.

/// The case of an identifier, judged after its leading underscores (`_privateThing` is
/// lowerCamelCase) and ignoring trailing ones (`value_` is one lowercase word).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    /// One lowercase word, digits allowed: `ply`, `x`, `v2`.
    Lower,
    /// Lowercase words joined by underscores: `my_var`.
    Snake,
    /// Starts with a lowercase letter and contains an uppercase one: `myVar`, `steamID`.
    Camel,
    /// Starts with an uppercase letter and contains a lowercase one: `PlayerSpawn`, `SteamID`.
    Pascal,
    /// Uppercase letters, digits and underscores only: `SERVER`, `MAX_PLAYERS`, `X`.
    Screaming,
    /// Anything else: `_`, the empty name, names starting with a digit or a non-ASCII letter.
    Other,
}

/// Classifies `name`.
pub fn classify(name: &str) -> Case {
    let trimmed = name.trim_start_matches('_').trim_end_matches('_');
    let Some(first) = trimmed.chars().next() else {
        return Case::Other;
    };
    let has_upper = trimmed.contains(|c: char| c.is_ascii_uppercase());
    let has_lower = trimmed.contains(|c: char| c.is_ascii_lowercase());
    if first.is_ascii_lowercase() {
        if has_upper {
            Case::Camel
        } else if trimmed.contains('_') {
            Case::Snake
        } else {
            Case::Lower
        }
    } else if first.is_ascii_uppercase() {
        if has_lower {
            Case::Pascal
        } else {
            Case::Screaming
        }
    } else {
        Case::Other
    }
}

/// Whether `name` is lowerCamelCase: after any leading underscores it starts with a lowercase
/// ASCII letter and it contains an uppercase ASCII letter.
///
/// snake_case, SCREAMING_CASE and ConstantStyle names are not lowerCamelCase.
pub fn is_lower_camel_case(name: &str) -> bool {
    classify(name) == Case::Camel
}

/// A naming style a naming reader enforces, from its `EnforcedStyle` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamingStyle {
    /// `snake_case`: lowerCamelCase names are flagged; snake_case, single lowercase words,
    /// PascalCase and SCREAMING_SNAKE_CASE are accepted.
    Snake,
    /// `camelCase`: snake_case names are flagged; lowerCamelCase, single lowercase words,
    /// PascalCase and SCREAMING_SNAKE_CASE are accepted.
    Camel,
    /// `PascalCase`: names starting with a lowercase letter (single words included) are flagged;
    /// PascalCase and SCREAMING_SNAKE_CASE are accepted.
    Pascal,
}

impl NamingStyle {
    /// The style named by an `EnforcedStyle` value; unknown values mean `snake_case`.
    pub fn from_option(value: &str) -> Self {
        match value {
            "camelCase" => NamingStyle::Camel,
            "PascalCase" => NamingStyle::Pascal,
            _ => NamingStyle::Snake,
        }
    }

    /// The name of the style as written in messages and configuration.
    pub fn label(self) -> &'static str {
        match self {
            NamingStyle::Snake => "snake_case",
            NamingStyle::Camel => "camelCase",
            NamingStyle::Pascal => "PascalCase",
        }
    }

    /// Whether `name` breaks the style.
    pub fn rejects(self, name: &str) -> bool {
        let case = classify(name);
        match self {
            NamingStyle::Snake => case == Case::Camel,
            NamingStyle::Camel => case == Case::Snake,
            NamingStyle::Pascal => matches!(case, Case::Lower | Case::Snake | Case::Camel),
        }
    }
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

    #[test]
    fn classifies_every_case() {
        let cases = [
            ("ply", Case::Lower),
            ("v2", Case::Lower),
            ("value_", Case::Lower),
            ("_x", Case::Lower),
            ("my_var", Case::Snake),
            ("myVar", Case::Camel),
            ("steamID", Case::Camel),
            ("PlayerSpawn", Case::Pascal),
            ("SteamID", Case::Pascal),
            ("Foo_bar", Case::Pascal),
            ("SERVER", Case::Screaming),
            ("MAX_PLAYERS", Case::Screaming),
            ("X", Case::Screaming),
            ("_G", Case::Screaming),
            ("_", Case::Other),
            ("", Case::Other),
            ("ñame", Case::Other),
        ];
        for (name, case) in cases {
            assert_eq!(classify(name), case, "{name}");
        }
    }

    #[test]
    fn styles_reject_the_opposite_cases() {
        let verdicts = |style: NamingStyle| -> Vec<bool> {
            ["ply", "my_var", "myVar", "PlayerSpawn", "MAX_PLAYERS", "_"]
                .iter()
                .map(|name| style.rejects(name))
                .collect()
        };
        assert_eq!(
            verdicts(NamingStyle::Snake),
            [false, false, true, false, false, false]
        );
        assert_eq!(
            verdicts(NamingStyle::Camel),
            [false, true, false, false, false, false]
        );
        assert_eq!(
            verdicts(NamingStyle::Pascal),
            [true, true, true, false, false, false]
        );
    }

    #[test]
    fn styles_come_from_options() {
        assert_eq!(NamingStyle::from_option("camelCase"), NamingStyle::Camel);
        assert_eq!(NamingStyle::from_option("PascalCase"), NamingStyle::Pascal);
        assert_eq!(NamingStyle::from_option("snake_case"), NamingStyle::Snake);
        assert_eq!(NamingStyle::from_option("nope"), NamingStyle::Snake);
        assert_eq!(NamingStyle::Camel.label(), "camelCase");
    }
}
