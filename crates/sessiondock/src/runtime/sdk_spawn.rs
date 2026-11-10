//! Capture the initiator while a direct local SDK create request is alive.
//! Shared SDK daemons cannot supply per-request ancestry after creation.
use super::procscan::SessionRow;
use crate::{error::ApiError, metadata::NestParent, state::AppState};
use process_links::linux;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
};

pub struct Initiator {
    parent: NestParent,
    identities: BTreeMap<(String, String), String>,
}

pub async fn capture(
    state: &AppState,
    peer: Option<SocketAddr>,
) -> Result<Option<Initiator>, ApiError> {
    let Some(peer) = peer.filter(|p| p.ip().is_loopback()) else {
        return Ok(None);
    };
    let Some(scanner) = state.proc_scan.clone() else {
        return Ok(None);
    };
    let Ok(document) = state.reader.run(|store| store.list(true)).await else {
        return Ok(None);
    };
    let rows = SessionRow::from_list(&document);
    // Process observation failure must not prevent an otherwise valid create.
    Ok(tokio::task::spawn_blocking(move || {
        let snapshot = linux::collect(scanner.root());
        let owners = linux::local_socket_owners(scanner.root(), peer, &snapshot);
        let [owner] = owners.as_slice() else {
            return None;
        };
        let scan = scanner.scan_blocking();
        let owned = scan.attribution_processes(&rows);
        let by_pid: BTreeMap<_, _> = owned
            .iter()
            .flat_map(|(uid, pids)| {
                pids.iter()
                    .filter(|pid| **pid > 0)
                    .map(move |pid| (*pid as u32, uid))
            })
            .collect();
        let by_key: BTreeMap<_, _> = rows
            .iter()
            .map(|r| ((r.source.clone(), r.sid.to_ascii_lowercase()), r))
            .collect();
        let stale = linux::launcher_identities(&snapshot, owner.pid, |e| {
            by_pid.contains_key(&e.process.pid)
        });
        let mut pid = owner.pid;
        let mut seen = BTreeSet::new();
        let mut parent = None;
        while pid > 1 && seen.insert(pid) {
            let entry = snapshot.entries.get(&pid)?;
            if entry.shared_parent {
                break;
            }
            parent = by_pid
                .get(&pid)
                .and_then(|uid| rows.iter().find(|r| r.uid.as_str() == uid.as_str()))
                .or_else(|| {
                    entry
                        .identities
                        .iter()
                        .filter(|key| !stale.contains(key))
                        .find_map(|key| by_key.get(key).copied())
                });
            if parent.is_some() {
                break;
            }
            pid = entry.parent;
        }
        let parent = parent?;
        if !linux::identity(scanner.root(), owner.pid).is_some_and(|(p, _)| p == *owner) {
            return None;
        }
        Some(Initiator {
            parent: NestParent {
                node_id: None,
                source: parent.source.clone(),
                sid: parent.sid.clone(),
            },
            identities: rows
                .into_iter()
                .map(|r| ((r.source, r.sid), r.uid))
                .collect(),
        })
    })
    .await
    .ok()
    .flatten())
}

pub fn record(
    state: &AppState,
    uid: &str,
    source: &str,
    sid: &str,
    initiator: Option<Initiator>,
) -> Result<(), ApiError> {
    if let Some(store) = &state.metadata {
        let previous = store.snapshot();
        let updated = if let Some(mut initiator) = initiator {
            initiator
                .identities
                .insert((source.to_owned(), sid.to_owned()), uid.to_owned());
            store.initialize_nest_parents(
                &[(uid.to_owned(), initiator.parent)],
                &initiator.identities,
            )?
        } else {
            store.initialize_nest_decision(uid)?
        };
        if !std::sync::Arc::ptr_eq(&previous, &updated) {
            state.ui_events.publish_sessions();
        }
        state.reader.store.invalidate_inventory();
    }
    Ok(())
}
