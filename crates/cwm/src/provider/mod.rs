//! The **provider bridge**: the NDJSON wire format and [`SubprocessProvider`],
//! which runs a provider as a child process so it can be written in any language.

mod subprocess;
mod wire;

pub use subprocess::SubprocessProvider;
pub use wire::{
    encode_request, parse_line, read_line_bounded, WireLine, WireReadError, MAX_LINE_BYTES,
    PROTOCOL_VERSION,
};
