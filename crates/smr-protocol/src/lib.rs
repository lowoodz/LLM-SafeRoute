pub mod body;
pub mod convert;
pub mod protocol;

pub use body::{
    extract_texts, inject_response_texts, inject_texts, parse_json_body, serialize_json_body,
    ExtractedText, TextPointer,
};
pub use convert::{anthropic_to_openai, openai_to_anthropic, target_path};
pub use protocol::{detect_protocol, ApiProtocol};
