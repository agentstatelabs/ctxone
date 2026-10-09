//! Chooses which Lens build `src/lens.rs` embeds.
//!
//! In the repository, the Hub embeds `../web/build/`, which `npm run build`
//! writes. A published crate cannot reach outside its own directory, so
//! `scripts/stage-lens.sh` copies that build into `lens/` before
//! `cargo publish`, and the package ships it. The repository build wins when it
//! exists, so a leftover `lens/` never shadows a fresh frontend build.

use std::path::Path;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(lens_staged)");
    println!("cargo::rerun-if-changed=build.rs");

    let repo_build = Path::new("../web/build/index.html");
    let staged = Path::new("lens/index.html");

    // The choice can only change while a staged copy exists, so watch the two
    // builds only then. Cargo reruns a build script on every build while a
    // watched path is missing, and that would rebuild the Hub every time.
    if staged.exists() {
        println!("cargo::rerun-if-changed=lens/index.html");
        println!("cargo::rerun-if-changed=../web/build/index.html");
        if !repo_build.exists() {
            println!("cargo::rustc-cfg=lens_staged");
        }
    }
}
