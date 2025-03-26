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

    pub const PONZHYLE_ELF: &[u8] = crate::methods::PONZHYLE_ELF;
    pub const PONZHYLE_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::PONZHYLE_ID);

    pub const PASSPORT_ELF: &[u8] = crate::methods::PASSPORT_ELF;
    pub const PASSPORT_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::PASSPORT_ID);

    pub const TWITTER_ELF: &[u8] = crate::methods::TWITTER_ELF;
    pub const TWITTER_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::TWITTER_ID);

    pub const STAKING_ELF: &[u8] = crate::methods::STAKING_ELF;
    pub const STAKING_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::STAKING_ID);

    pub const RISC0_RECURSION_ELF: &[u8] = crate::methods::RISC0_RECURSION_ELF;
    pub const RISC0_RECURSION_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::RISC0_RECURSION_ID);

    pub const UUID_TLD_ELF: &[u8] = crate::methods::UUID_TLD_ELF;
    pub const UUID_TLD_ID: [u8; 32] = sdk::to_u8_array(&crate::methods::UUID_TLD_ID);
}

#[cfg(any(clippy, not(feature = "nonreproducible")))]
mod metadata {
    pub const AMM_ELF: &[u8] = include_bytes!("amm/amm.img");
    pub const AMM_ID: [u8; 32] = sdk::str_to_u8(include_str!("amm/amm.txt"));

    pub const HYDENTITY_ELF: &[u8] = include_bytes!("hydentity/hydentity.img");
    pub const HYDENTITY_ID: [u8; 32] = sdk::str_to_u8(include_str!("hydentity/hydentity.txt"));

    pub const HYLLAR_ELF: &[u8] = include_bytes!("hyllar/hyllar.img");
    pub const HYLLAR_ID: [u8; 32] = sdk::str_to_u8(include_str!("hyllar/hyllar.txt"));

    pub const PONZHYLE_ELF: &[u8] = include_bytes!("ponzhyle/ponzhyle.img");
    pub const PONZHYLE_ID: [u8; 32] = sdk::str_to_u8(include_str!("ponzhyle/ponzhyle.txt"));

    pub const PASSPORT_ELF: &[u8] = include_bytes!("passport/passport.img");
    pub const PASSPORT_ID: [u8; 32] = sdk::str_to_u8(include_str!("passport/passport.txt"));

    pub const TWITTER_ELF: &[u8] = include_bytes!("twitter/twitter.img");
    pub const TWITTER_ID: [u8; 32] = sdk::str_to_u8(include_str!("twitter/twitter.txt"));

    pub const STAKING_ELF: &[u8] = include_bytes!("staking/staking.img");
    pub const STAKING_ID: [u8; 32] = sdk::str_to_u8(include_str!("staking/staking.txt"));

    pub const RISC0_RECURSION_ELF: &[u8] = include_bytes!("risc0-recursion/risc0-recursion.img");
    pub const RISC0_RECURSION_ID: [u8; 32] =
        sdk::str_to_u8(include_str!("risc0-recursion/risc0-recursion.txt"));

    pub const UUID_TLD_ELF: &[u8] = include_bytes!("uuid-tld/uuid-tld.img");
    pub const UUID_TLD_ID: [u8; 32] = sdk::str_to_u8(include_str!("uuid-tld/uuid-tld.txt"));
}

pub use metadata::*;
