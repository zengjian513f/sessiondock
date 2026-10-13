//! Optional browser integrations shared by nodes and the Hub.
use std::{env, io};

use serde_json::{Value, json};

#[derive(Clone, Debug, Default)]
pub struct Integrations {
    pub labdesk_url: Option<String>,
    pub filedock_url: Option<String>,
}

impl Integrations {
    pub fn from_env() -> io::Result<Self> {
        fn read(name: &str) -> io::Result<Option<String>> {
            match env::var(name) {
                Ok(value) if value.trim().is_empty() => Ok(None),
                Ok(value) => Ok(Some(value.trim().to_owned())),
                Err(env::VarError::NotPresent) => Ok(None),
                Err(error) => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name}: {error}"),
                )),
            }
        }
        Ok(Self {
            labdesk_url: read("SESSIONDOCK_LABDESK_URL")?,
            filedock_url: read("SESSIONDOCK_FILEDOCK_URL")?,
        })
    }

    pub fn apply(&self, capabilities: &mut Value) {
        capabilities["research"] = json!(self.labdesk_url.is_some());
        capabilities["file_browser"] = json!(self.filedock_url.is_some());
        capabilities["labdesk_url"] = json!(self.labdesk_url);
        capabilities["filedock_url"] = json!(self.filedock_url);
    }
}
