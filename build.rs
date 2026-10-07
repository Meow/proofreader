/// Rebuilds the crate whenever a file under `src/readers` is added or removed, which the
/// `automod::dir!` expansions cannot track on their own.
fn main() {
    println!("cargo:rerun-if-changed=src/readers");
}
