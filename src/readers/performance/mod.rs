//! Performance readers: code that costs more than it needs to, such as expensive calls in
//! functions that run every frame or tick.

pub mod constant_object_in_hot_path;
pub mod expensive_call_in_hot_path;
pub mod hot_paths;
