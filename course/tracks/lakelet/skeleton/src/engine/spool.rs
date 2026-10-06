//! The write path as it is before there is a log: an append-only file of accepted payloads.
//!
//! Step `lakelet/4.1-ingest-protocol` gives this file its shape — one length-prefixed record per
//! accepted frame, in the order the frames were accepted — and the steps after it harden exactly
//! this file rather than inventing a second one:
//!
//! * `4.3` decides when it is flushed to the device (`WalPolicy`) and writes the decision down;
//! * `4.4` gives each record its own checksum, so a process killed mid-write leaves something the
//!   next start can recognise as a torn tail instead of a plausible record;
//! * `4.5` starts writing batches into it, and counts what that costs.
//!
//! The contract a spec leans on is small on purpose: [`Spool::append`] may only report success once
//! the record is readable by [`Spool::records`], and [`Spool::records`] answers what was appended, in
//! order, never what was appended and then lost to a bug in the reader.

use std::path::{Path, PathBuf};

use crate::error::Error;

/// The name of the file inside the store directory.
pub const FILE: &str = "ingest.log";

/// The append-only write path.
#[derive(Debug, Clone)]
pub struct Spool {
    /// Directory this spool owns (`store.dir` in the configuration).
    directory: PathBuf,
}

impl Spool {
    /// Open (or create) the store directory.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the directory cannot be created or written to.
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self, Error> {
        let _ = directory.into();
        todo!("step lakelet/4.1-ingest-protocol")
    }

    /// The directory this spool writes to.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The file the records land in.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.directory.join(FILE)
    }

    /// Append one accepted payload, and answer only once it can be read back.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the record cannot be written.
    pub fn append(&mut self, payload: &[u8]) -> Result<(), Error> {
        let _ = payload;
        todo!("step lakelet/4.1-ingest-protocol")
    }

    /// Every record the file holds, in the order it was appended.
    ///
    /// # Errors
    ///
    /// [`Error::Store`] when the file cannot be read, and [`Error::Protocol`] when what is in it is
    /// not the record format this module writes — a reader that guesses is how a corrupt store
    /// becomes a corrupt *answer*.
    pub fn records(&self) -> Result<Vec<Vec<u8>>, Error> {
        todo!("step lakelet/4.1-ingest-protocol")
    }
}
