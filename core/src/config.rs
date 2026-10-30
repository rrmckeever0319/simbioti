use crate::error::{Error, Result};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Hub,
    Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub role: Role,
    pub folder: PathBuf,
    #[serde(default)]
    pub listen: Option<String>,
    #[serde(default)]
    pub peer: Option<String>,
    pub psk_hex: String,
}

impl Config {
    pub fn new(
        role: Role,
        folder: PathBuf,
        listen: Option<String>,
        peer: Option<String>,
        psk_hex: String,
    ) -> Result<Self> {
        let cfg = Self {
            role,
            folder,
            listen,
            peer,
            psk_hex,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        let cfg: Self = toml::from_str(&text).map_err(|err| Error::Config(err.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        let _ = self.psk()?;
        match self.role {
            Role::Hub => {
                if self.listen.as_deref().unwrap_or("").is_empty() {
                    return Err(Error::Config("hub requires listen".into()));
                }
            }
            Role::Client => {
                if self.peer.as_deref().unwrap_or("").is_empty() {
                    return Err(Error::Config("client requires peer".into()));
                }
            }
        }
        Ok(())
    }

    pub fn psk(&self) -> Result<[u8; 32]> {
        parse_psk(&self.psk_hex)
    }
}

pub fn parse_psk(text: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(text.trim()).map_err(|err| Error::Config(err.to_string()))?;
    if bytes.len() != 32 {
        return Err(Error::Config(format!(
            "psk must be 32 bytes (64 hex chars), got {} bytes",
            bytes.len()
        )));
    }
    let mut psk = [0u8; 32];
    psk.copy_from_slice(&bytes);
    Ok(psk)
}
