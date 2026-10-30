#![forbid(unsafe_code)]

mod config;
mod crypto;
mod error;
mod index;
mod media;
mod paths;
mod proto;
mod sync;

pub use config::{Config, Role};
pub use error::{Error, Result};
pub use media::is_media;
pub use sync::{
    run_client, scan_library, session_as_client, session_as_hub, HubServer, SyncReport,
};
