use wasm_bindgen::JsValue;

#[derive(Debug)]
pub struct VerifierError(pub String);

// See https://rustwasm.github.io/wasm-bindgen/reference/types/result.html
impl Into<JsValue> for VerifierError {
    fn into(self) -> JsValue {
        JsValue::from_str("runner failed")
    }
}

impl<T: std::error::Error> From<T> for VerifierError {
    fn from(error: T) -> Self {
        VerifierError(error.to_string())
    }
}
