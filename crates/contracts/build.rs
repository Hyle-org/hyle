// Deactivate in clippy, we don't really need to recompile and it breaks our CI.
#[cfg(any(clippy, not(feature = "build")))]
fn main() {
    use std::fs;
    use std::path::Path;
    use std::env;

    // Get the OUT_DIR from Cargo
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = Path::new(&out_dir);

    // Get the workspace directory (2 levels up from the crate directory)
    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let workspace_path = Path::new(&crate_dir)
        .parent()
        .expect("Failed to get parent directory")
        .parent()
        .expect("Failed to get parent directory")
        .parent()
        .expect("Failed to get workspace directory");

    // List of directories to check
    let dirs = [
        "amm",
        "hydentity",
        "hyllar",
        "staking",
        "risc0-recursion",
        "uuid-tld",
    ];

    // Copy .img and .txt files from each directory
    for dir in dirs.iter() {
        let dir_path = workspace_path.join("crates/contracts").join(dir);
        if dir_path.exists() {
            // Create the target directory if it doesn't exist
            let target_dir = out_path.join(dir);
            fs::create_dir_all(&target_dir).expect("Failed to create target directory");

            // Copy .img files
            if let Ok(entries) = fs::read_dir(dir_path) {
                for entry in entries {
                    if let Ok(entry) = entry {
                        let path = entry.path();
                        if path.extension().map_or(false, |ext| ext == "img" || ext == "txt") {
                            let file_name = path.file_name().unwrap().to_str().unwrap();
                            let target_path = target_dir.join(file_name);
                            println!("cargo:warning=Copying {} to {}", path.display(), target_path.display());
                            fs::copy(&path, &target_path).expect("Failed to copy file");
                        }
                    }
                }
            }
        } else {
            println!("cargo:warning=Directory {} does not exist", dir_path.display());
        }
    }
}

#[cfg(all(not(clippy), feature = "build"))]
fn main() {
    // clippy in workspace mode sets this, which interferes with the guest VM. Clear it temporarily.
    let env_wrapper = std::env::var("RUSTC_WORKSPACE_WRAPPER");
    std::env::set_var("RUSTC_WORKSPACE_WRAPPER", "");

    println!("cargo:rerun-if-changed=build.rs");

    use risc0_build::{DockerOptionsBuilder, GuestOptionsBuilder};
    use std::collections::HashMap;

    let reproducible = cfg!(not(feature = "nonreproducible"));

    let mut options = HashMap::new();

    [
        "hyle-amm",
        "hyle-hydentity",
        "hyle-hyllar",
        "hyle-staking",
        "hyle-risc0-recursion",
        "hyle-uuid-tld",
    ]
    .iter()
    .for_each(|name| {
        let mut guest_opts = GuestOptionsBuilder::default();

        guest_opts.features(vec!["risc0".into()]);

        if reproducible {
            guest_opts.use_docker(
                DockerOptionsBuilder::default()
                    // Point to the workspace
                    .root_dir("../..".to_string())
                    .build()
                    .unwrap(),
            );
        }

        options.insert(*name, guest_opts.build().unwrap());
    });

    // Build the guests.
    let results = risc0_build::embed_methods_with_options(options);

    if reproducible {
        results.iter().for_each(|data| {
            std::fs::write(format!("{}/{}.img", data.name, data.name), &data.elf)
                .expect("failed to write img");
            // Convert u32 slice to hex
            let hex_image_id = data
                .image_id
                .as_words()
                .iter()
                .map(|x| format!("{:08x}", x.to_be()))
                .collect::<Vec<_>>()
                .join("");
            std::fs::write(format!("{}/{}.txt", data.name, data.name), &hex_image_id)
                .expect("failed to write program ID");
        });
    }
    std::env::set_var("RUSTC_WORKSPACE_WRAPPER", env_wrapper.unwrap_or_default());
}
