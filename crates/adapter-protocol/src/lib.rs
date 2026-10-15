//! Switchyard adapter protocol (see README.md).

mod adapter;
pub mod conformance;
pub mod echo;
mod protocol;

pub use adapter::*;
pub use protocol::*;
