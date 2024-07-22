use utils::{error::VerifierError, prove};
use wasm_bindgen::prelude::*;

pub mod utils;

extern crate web_sys;

// A macro to provide `println!(..)`-style syntax for `console.log` logging.
macro_rules! log {
    ( $( $t:tt )* ) => {
        web_sys::console::log_1(&format!( $( $t )* ).into());
    }
}

#[wasm_bindgen]
pub fn wasm_prove(
    trace_data: Vec<u8>,
    memory_data: Vec<u8>,
    output: &str,
) -> Result<JsValue, VerifierError> {
    // Sets up panic for easy debugging
    std::panic::set_hook(Box::new(console_error_panic_hook::hook));

    let proof = prove(&trace_data, &memory_data, output)?;
    Ok(serde_wasm_bindgen::to_value(&proof).unwrap())
}
