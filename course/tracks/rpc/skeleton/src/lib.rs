//! A small RPC layer: messages, deadlines, statuses and streams.
//!
//! Every step of this track adds to the library; the binary stays a shell. There is no network
//! here: the contract is data, the deadline is a number, and the stream is a window.

// The region later scaffolds are allowed to write into (authoring.md §5). A scaffold may only
// *insert* lines inside it, and only *remove* nothing.
// kiln:scaffold:begin
// kiln:scaffold:end

use std::collections::BTreeMap;

/// A field's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A string.
    Text,
    /// A number.
    Number,
    /// A flag.
    Flag,
}

/// One field of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The wire number.
    pub number: u32,
    /// The name, which the wire never carries.
    pub name: String,
    /// The type the number carries.
    pub kind: Kind,
}

/// A change that breaks a client of the old message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Breaking {
    /// A number is gone and a client may still send it.
    NumberRemoved(u32),
    /// A number now carries another type.
    NumberReused(u32),
}

/// Every breaking difference between two messages, sorted by number.
#[must_use]
pub fn wire(old: &[Field], new: &[Field]) -> Vec<Breaking> {
    todo!("by number: removed, or the same number with another type")
}

/// The version of the service.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
