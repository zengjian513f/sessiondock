//! Offline transfer inspection/staging tool. No native publish or source cleanup.
//! One JSON request on stdin, one JSON result on stdout. All roots are explicit.

use serde::Deserialize;
use sessiondock::{
    sessions::{SessionRoots, SessionStore},
    transfer::{TransferError, codex, environment, files, group},
};
use std::{io::Read, path::PathBuf};

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Request {
    InspectEnvironment {
        cwd: PathBuf,
        #[serde(default)]
        dependencies: Vec<PathBuf>,
    },
    CompareEnvironment {
        snapshot: environment::Snapshot,
    },
    RecheckEnvironment {
        snapshot: environment::Snapshot,
    },
    CreateStorageProbe {
        root: PathBuf,
    },
    CheckStorageProbe {
        root: PathBuf,
        probe: environment::StorageProbe,
    },
    RemoveStorageProbe {
        root: PathBuf,
        probe: environment::StorageProbe,
    },
    Group {
        uid: String,
        roots: Roots,
    },
    PlanCodex {
        uid: String,
        roots: Roots,
        mode: codex::Mode,
    },
    PlanFiles {
        uid: String,
        roots: Roots,
        new_ids: bool,
    },
    StageFiles {
        plan: files::Plan,
        destination: PathBuf,
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
    agy: Option<PathBuf>,
}

impl From<Roots> for SessionRoots {
    fn from(r: Roots) -> Self {
        Self {
            claude: r.claude,
            codex: r.codex,
            grok: r.grok,
            opencode: r.opencode,
            agy: r.agy,
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
        Request::InspectEnvironment { cwd, dependencies } => {
            serde_json::to_value(environment::Snapshot::capture(&cwd, &dependencies)?)?
        }
        Request::CompareEnvironment { snapshot } => {
            let paths = snapshot.dependencies.keys().cloned().collect::<Vec<_>>();
            let target = environment::Snapshot::capture(&snapshot.cwd, &paths)?;
            snapshot.compare(&target)?;
            serde_json::json!({"matches":true,"snapshot":target})
        }
        Request::RecheckEnvironment { snapshot } => {
            snapshot.recheck()?;
            serde_json::json!({"unchanged":true})
        }
        Request::CreateStorageProbe { root } => {
            serde_json::to_value(environment::StorageProbe::create(&root)?)?
        }
        Request::CheckStorageProbe { root, probe } => {
            serde_json::json!({"shared":probe.shared(&root)?})
        }
        Request::RemoveStorageProbe { root, probe } => {
            probe.remove(&root)?;
            serde_json::json!({"removed":true})
        }
        Request::Group { uid, roots } => serde_json::to_value(derive(&uid, roots)?)?,
        Request::PlanCodex { uid, roots, mode } => {
            serde_json::to_value(codex::plan(derive(&uid, roots)?, mode)?)?
        }
        Request::PlanFiles {
            uid,
            roots,
            new_ids,
        } => serde_json::to_value(files::Plan::build(derive(&uid, roots)?, new_ids)?)?,
        Request::StageFiles { plan, destination } => {
            plan.stage(&destination)?;
            serde_json::json!({"staged":true,"publishable":false})
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
