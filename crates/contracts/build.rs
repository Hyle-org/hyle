// Deactivate in clippy, we don't really need to recompile and it breaks our CI.
#[cfg(any(clippy, not(feature = "build")))]
fn main() {}

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
