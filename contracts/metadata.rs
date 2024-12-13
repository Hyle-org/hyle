#[allow(unused)]
#[cfg(all(not(clippy), feature = "build"))]
mod methods {
    include!(concat!(env!("OUT_DIR"), "/methods.rs"));
}

#[cfg(all(not(clippy), feature = "build"))]
mod metadata {
    pub const AMM_ELF: &[u8] = crate::methods::AMM_ELF;
    pub const AMM_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::AMM_ID);

    pub const HYDENTITY_ELF: &[u8] = crate::methods::HYDENTITY_ELF;
    pub const HYDENTITY_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::HYDENTITY_ID);

    pub const HYLLAR_ELF: &[u8] = crate::methods::HYLLAR_ELF;
    pub const HYLLAR_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::HYLLAR_ID);

    pub const STAKING_ELF: &[u8] = crate::methods::STAKING_ELF;
    pub const STAKING_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::STAKING_ID);
}

#[cfg(any(clippy, not(feature = "build")))]
mod metadata {
    pub const AMM_ELF: &[u8] = include_bytes!("amm/amm.img");
    pub const AMM_ID: [u8; 32] = sdk::str_to_u8(include_str!("amm/amm.txt"));

    pub const HYDENTITY_ELF: &[u8] = include_bytes!("hydentity/hydentity.img");
    pub const HYDENTITY_ID: [u8; 32] = sdk::str_to_u8(include_str!("hydentity/hydentity.txt"));

    pub const HYLLAR_ELF: &[u8] = include_bytes!("hyllar/hyllar.img");
    pub const HYLLAR_ID: [u8; 32] = sdk::str_to_u8(include_str!("hyllar/hyllar.txt"));

    pub const STAKING_ELF: &[u8] = include_bytes!("staking/staking.img");
    pub const STAKING_ID: [u8; 32] = sdk::str_to_u8(include_str!("staking/staking.txt"));
}

pub use metadata::*;
