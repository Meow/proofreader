//! Names of Garry's Mod libraries, shared by readers that check for shadowing.

/// Global GMod library tables that variables and parameters must not shadow.
pub const GMOD_LIBRARIES: &[&str] = &[
    "player", "team", "file", "table", "sound", "string", "math", "util", "net", "hook", "timer",
    "render", "surface", "draw", "ents", "game", "engine", "input", "gui", "vgui", "http", "os",
    "debug", "bit",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_names_are_unique() {
        let mut names = GMOD_LIBRARIES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), GMOD_LIBRARIES.len());
    }
}
