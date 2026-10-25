//! `Lint/UnusedSelf`.

use crate::reader::{Context, Reader, Registration};
use crate::readers::lint::scopes::resolve;

/// Flags `function a:b()` methods whose implicit `self` is never read, which could be defined
/// with `.` instead.
///
/// Disabled by default: GMod hooks and scripted classes (`function GM:PlayerSpawn(ply)`,
/// `function ENT:Initialize()`) are methods by convention whether or not they use `self`. There
/// is no fix, as switching to `.` changes how the function must be called. Files that do not
/// parse are skipped.
pub struct UnusedSelf;

impl Reader for UnusedSelf {
    fn name(&self) -> &'static str {
        "Lint/UnusedSelf"
    }

    fn description(&self) -> &'static str {
        "Checks for methods that never use their implicit `self`."
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn investigate(&self, ctx: &mut Context) {
        let Some(resolution) = resolve(ctx.source) else {
            return;
        };
        for variable in &resolution.variables {
            if variable.implicit && variable.reads == 0 {
                ctx.add_offense(
                    variable.token.range(),
                    "`self` is never used; define the method with `.` instead of `:`.",
                );
            }
        }
    }
}

inventory::submit! { Registration(|| Box::new(UnusedSelf)) }

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::config::Config;
    use crate::testing::*;

    const READER: &str = "Lint/UnusedSelf";
    const MESSAGE: &str = "`self` is never used; define the method with `.` instead of `:`.";

    #[test]
    fn flags_methods_without_self() {
        expect_offense(READER, "function a:b(x) return x end\n", 1, 12, MESSAGE);
        expect_offense(
            READER,
            "function a.b.c:d()\n  local f = function(self) return self end\nend\n",
            1,
            16,
            MESSAGE,
        );
    }

    #[test]
    fn accepts_methods_using_self() {
        expect_no_offenses(
            READER,
            "function a:b() return self.x end\nfunction a:c() return function() print(self) end end\n\
             function a.d(x) end\nlocal function e() end\n",
        );
    }

    #[test]
    fn is_disabled_by_default() {
        let config = Config::defaults(Path::new("."));
        assert!(!config.reader(READER).enabled);
    }

    #[test]
    fn never_autocorrects() {
        let src = "function a:b() end\n";
        assert_eq!(autocorrect(READER, src), src);
    }
}
