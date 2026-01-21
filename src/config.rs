use crate::{error::BuildonomyError, properties::BeliefNode, query::Focus};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::{
    fs::{read_to_string, write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Global static variable to hold the config provider.
pub static CONFIG_PROVIDER: OnceCell<Mutex<Arc<dyn LatticeConfigProvider>>> = OnceCell::new();

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct NetworkRecord {
    pub path: String,
    pub node: BeliefNode,
}

#[uniffi::export(with_foreign)]
pub trait LatticeConfigProvider: Send + Sync {
    fn get_networks(&self) -> Result<Vec<NetworkRecord>, BuildonomyError>;
    fn set_networks(&self, nets: Vec<NetworkRecord>) -> Result<(), BuildonomyError>;
    fn get_focus(&self) -> Result<Focus, BuildonomyError>;
    fn set_focus(&self, focus: Focus) -> Result<(), BuildonomyError>;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TomlConfigProvider {
    path: PathBuf,
}

impl TomlConfigProvider {
    pub fn new(path: PathBuf) -> Self {
        TomlConfigProvider { path }
    }
}

impl LatticeConfigProvider for TomlConfigProvider {
    fn get_networks(&self) -> Result<Vec<NetworkRecord>, BuildonomyError> {
        tracing::debug!("Attempting to read networks from: {:?}", &self.path);
        if !self.path.exists() {
            tracing::debug!("Config file not found, returning empty network list.");
            return Ok(Vec::new());
        }
        let content = read_to_string(&self.path)?;
        let config: BTreeMap<String, Vec<NetworkRecord>> = toml::from_str(&content)?;
        config
            .get("networks")
            .cloned()
            .ok_or_else(|| BuildonomyError::NotFound("networks not found in config".to_string()))
    }

    fn set_networks(&self, nets: Vec<NetworkRecord>) -> Result<(), BuildonomyError> {
        tracing::debug!("Attempting to write networks to: {:?}", &self.path);
        let mut config = BTreeMap::new();
        config.insert("networks".to_string(), nets);
        let toml_string = toml::to_string(&config)?;
        write(&self.path, toml_string)?;
        Ok(())
    }

    fn get_focus(&self) -> Result<Focus, BuildonomyError> {
        let content = read_to_string(&self.path)?;
        let config: BTreeMap<String, Focus> = toml::from_str(&content)?;
        config
            .get("focus")
            .cloned()
            .ok_or_else(|| BuildonomyError::NotFound("focus not found in config".to_string()))
    }

    fn set_focus(&self, focus: Focus) -> Result<(), BuildonomyError> {
        let mut config = BTreeMap::new();
        config.insert("focus".to_string(), focus);
        let toml_string = toml::to_string(&config)?;
        write(&self.path, toml_string)?;
        Ok(())
    }
}

pub fn get_content<P: AsRef<Path>>(path: P) -> Result<String, BuildonomyError> {
    tracing::debug!("Reading {:?}", path.as_ref());
    Ok(read_to_string(path)?)
}

pub async fn set_content<P: AsRef<Path>>(path: P, text: String) -> Result<(), BuildonomyError> {
    Ok(write(path, text)?)
}
