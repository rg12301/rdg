//! Registers every full-colour logo in `assets/color/` (see `THIRD_PARTY_LICENSES.md`):
//! each `<key>.svg` becomes an entry of `COLOR_ICONS`, so adding a logo is just adding
//! the file.

use std::fs;
use std::path::Path;

fn main() {
    let dir = Path::new("assets/color");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut keys: Vec<String> = fs::read_dir(dir)
        .expect("assets/color")
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            (p.extension()? == "svg").then(|| p.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    keys.sort();
    let mut out = String::from("/// `(key, standalone SVG document)` for every full-colour logo, sorted by key.\n");
    out.push_str("pub static COLOR_ICONS: &[(&str, &str)] = &[\n");
    for k in &keys {
        println!("cargo:rerun-if-changed=assets/color/{k}.svg");
        out.push_str(&format!(
            "    ({k:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/assets/color/{k}.svg\"))),\n"
        ));
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("color_icons.rs");
    fs::write(dest, out).unwrap();
}
