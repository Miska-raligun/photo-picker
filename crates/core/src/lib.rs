//! photo-pick core: ingest → features → group → output.

pub mod error;
pub mod features;
pub mod group;
pub mod ingest;
pub mod models;
pub mod output;
pub mod pipeline;
pub mod scoring;

pub use error::{Error, Result};
