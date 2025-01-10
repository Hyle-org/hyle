#[allow(unused)]
#[cfg(all(not(clippy), feature = "nonreproducible"))]
mod methods {
    include!(concat!(env!("OUT_DIR"), "/methods.rs"));
}

#[cfg(all(not(clippy), feature = "nonreproducible"))]
mod metadata {
    pub const AMM_ELF: &[u8] = crate::methods::AMM_ELF;
    pub const AMM_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::AMM_ID);

    pub const HYDENTITY_ELF: &[u8] = crate::methods::HYDENTITY_ELF;
    pub const HYDENTITY_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::HYDENTITY_ID);

    pub const HYLLAR_ELF: &[u8] = crate::methods::HYLLAR_ELF;
    pub const HYLLAR_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::HYLLAR_ID);

    pub const STAKING_ELF: &[u8] = crate::methods::STAKING_ELF;
    pub const STAKING_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::STAKING_ID);

    pub const RISC0_RECURSION_ELF: &[u8] = crate::methods::RISC0_RECURSION_ELF;
    pub const RISC0_RECURSION_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::RISC0_RECURSION_ID);
}

#[cfg(any(clippy, not(feature = "nonreproducible")))]
mod metadata {
    pub const AMM_ELF: &[u8] = include_bytes!("amm/amm.img");
    pub const AMM_ID: [u8; 32] = sdk::str_to_u8(include_str!("amm/amm.txt"));

    pub const HYDENTITY_ELF: &[u8] = include_bytes!("hydentity/hydentity.img");
    pub const HYDENTITY_ID: [u8; 32] = sdk::str_to_u8(include_str!("hydentity/hydentity.txt"));

    pub const HYLLAR_ELF: &[u8] = include_bytes!("hyllar/hyllar.img");
    pub const HYLLAR_ID: [u8; 32] = sdk::str_to_u8(include_str!("hyllar/hyllar.txt"));

    pub const STAKING_ELF: &[u8] = include_bytes!("staking/staking.img");
    pub const STAKING_ID: [u8; 32] = sdk::str_to_u8(include_str!("staking/staking.txt"));

    pub const RISC0_RECURSION_ELF: &[u8] = include_bytes!("risc0-recursion/risc0-recursion.img");
    pub const RISC0_RECURSION_ID: [u8; 32] =
        sdk::str_to_u8(include_str!("risc0-recursion/risc0-recursion.txt"));
}

pub use metadata::*;
