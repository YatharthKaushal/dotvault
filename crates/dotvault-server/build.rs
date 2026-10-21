// `sqlx::migrate!` embeds the files in `migrations/` at compile time; make
// cargo notice when one of them changes.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
