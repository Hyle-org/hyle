// Deactivate in clippy, we don't really need to recompile and it breaks our CI.
#[cfg(any(clippy, not(feature = "build")))]
fn main() {}

use cargo_manifest::Manifest;

#[allow(unused)]
fn retain_guests(manifest: &[u8], guests: &[&str]) -> Manifest {
    let mut manifest = Manifest::from_slice(manifest).expect("failed to parse Cargo.toml");

    manifest
        .package
        .as_mut()
        .unwrap()
        .metadata
        .as_mut()
        .unwrap()
        .get_mut("risc0")
        .unwrap()
        .get_mut("methods")
        .unwrap()
        .as_array_mut()
        .unwrap()
        .retain(|method| guests.contains(&method.as_str().unwrap()));
    manifest
}

#[cfg(all(not(clippy), feature = "build"))]
fn main() {
    // clippy in workspace mode sets this, which interferes with the guest VM. Clear it temporarily.
    let env_wrapper = std::env::var("RUSTC_WORKSPACE_WRAPPER");
    std::env::set_var("RUSTC_WORKSPACE_WRAPPER", "");

    println!("cargo:rerun-if-changed=build.rs");

    use risc0_build::{DockerOptions, GuestOptions};
    use std::collections::HashMap;

    let reproducible = cfg!(not(feature = "nonreproducible"));

    let mut options = HashMap::new();

    let build = [
        #[cfg(feature = "amm")]
        "amm",
        #[cfg(feature = "hydentity")]
        "hydentity",
        #[cfg(feature = "hyllar")]
        "hyllar",
        #[cfg(feature = "staking")]
        "staking",
        #[cfg(feature = "risc0-recursion")]
        "risc0-recursion",
    ];

    build.iter().for_each(|name| {
        options.insert(
            *name,
            GuestOptions {
                features: vec!["risc0".to_owned()],
                use_docker: match reproducible {
                    true => Some(DockerOptions {
                        // Point to the workspace
                        root_dir: Some("..".into()),
                    }),
                    false => None,
                },
            },
        );
    });

    #[cfg(not(feature = "all_guests"))]
    let results = {
        // HACK until risc0 supports this: ppen our own cargo.toml, tweak the risc0 methods, then save it back.
        let cargo_toml = std::fs::read("Cargo.toml").expect("failed to read Cargo.toml");

        // Retain only the guests we're building and save back to disk.
        let manifest = retain_guests(&cargo_toml, &build);

        std::fs::write(
            "Cargo.toml",
            toml::to_string(&manifest).expect("failed to serialize Cargo.toml"),
        )
        .expect("failed to write Cargo.toml");

        // Build the guests.
        let results = risc0_build::embed_methods_with_options(options);

        // Revert the Cargo.toml back to its original state.
        std::fs::write(
            "Cargo.toml",
            std::str::from_utf8(&cargo_toml).expect("failed to write Cargo.toml"),
        )
        .expect("failed to write Cargo.toml");

        results
    };

    // Build the guests.
    #[cfg(feature = "all_guests")]
    let results = risc0_build::embed_methods_with_options(options);

    if reproducible {
        results.iter().for_each(|data| {
            std::fs::write(format!("{}/{}.img", data.name, data.name), &data.elf)
                .expect("failed to write img");
            // Convert u32 slice to hex
            let hex_image_id = data
                .image_id
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
