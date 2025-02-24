use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

fn main() {
    // Lire le fichier Cargo.toml
    let mut cargo_toml_content = String::new();
    File::open("Cargo.toml")
        .unwrap()
        .read_to_string(&mut cargo_toml_content)
        .unwrap();

    let cargo_toml = cargo_toml_parser::parse_cargo_toml(&cargo_toml_content).unwrap();

    // Chemin vers le fichier généré
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("dependency_versions.rs");

    // Écrire les versions dans un fichier
    let mut f = File::create(&dest_path).unwrap();
    writeln!(
        f,
        "pub const RISC0_VERSION: &str = \"risc0-{}\";",
        cargo_toml.get_dependency("risc0-zkvm").unwrap().version
    )
    .unwrap();
    writeln!(
        f,
        "pub const SP1_VERSION: &str = \"sp1-{}\";",
        cargo_toml.get_dependency("sp1-sdk").unwrap().version
    )
    .unwrap();
}
