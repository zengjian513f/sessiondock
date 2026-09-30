//! Offline transfer inspection/staging tool. No native publish or source cleanup.
//! One JSON request on stdin, one JSON result on stdout. All roots are explicit.

use serde::Deserialize;
use sessiondock::{
    sessions::{SessionRoots, SessionStore},
    transfer::{TransferError, codex, group},
};
use std::{io::Read, path::PathBuf};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    Group {
        uid: String,
        roots: Roots,
    },
    PlanCodex {
        uid: String,
        roots: Roots,
        mode: codex::Mode,
    },
    StageCodex {
        plan: codex::ClonePlan,
        destination: PathBuf,
    },
}

#[derive(Deserialize)]
#[serde(default)]
#[derive(Default)]
struct Roots {
    claude: Option<PathBuf>,
    codex: Option<PathBuf>,
    grok: Option<PathBuf>,
    opencode: Option<PathBuf>,
}

impl From<Roots> for SessionRoots {
    fn from(r: Roots) -> Self {
        Self {
            claude: r.claude,
            codex: r.codex,
            grok: r.grok,
            opencode: r.opencode,
        }
    }
}

fn derive(uid: &str, roots: Roots) -> Result<group::Group, TransferError> {
    let store = SessionStore::new(roots.into());
    let snapshot = store
        .search_snapshot()
        .map_err(|e| TransferError::new("move_inventory", e.message))?;
    group::derive(&snapshot, uid)
}

fn run() -> Result<serde_json::Value, TransferError> {
    let mut raw = Vec::new();
    std::io::stdin().read_to_end(&mut raw)?;
    let request: Request = serde_json::from_slice(&raw)?;
    Ok(match request {
        Request::Group { uid, roots } => serde_json::to_value(derive(&uid, roots)?)?,
        Request::PlanCodex { uid, roots, mode } => {
            serde_json::to_value(codex::plan(derive(&uid, roots)?, mode)?)?
        }
        Request::StageCodex { plan, destination } => {
            serde_json::to_value(codex::stage(&plan, &destination)?)?
        }
    })
}

fn main() {
    match run() {
        Ok(value) => println!("{}", value),
        Err(error) => {
            println!("{}", serde_json::json!({"error": error}));
            std::process::exit(1);
        }
    }
}
