//! Performance readers: code that costs more than it needs to, such as expensive calls in
//! functions that run every frame or tick. The `HotPaths` and `HookFunctions` options of the
//! `Performance` section are shared by every reader that looks at hot paths.

pub mod constant_object_in_hot_path;
pub mod distance_comparison;
pub mod division_by_constant;
pub mod expensive_call_in_hot_path;
pub mod get_all_loop;
pub mod hot_paths;
pub mod sequential_pairs;
pub mod type_comparison;
