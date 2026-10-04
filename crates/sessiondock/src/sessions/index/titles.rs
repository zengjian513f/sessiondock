//! Point reads of current display titles. Only initial discovery builds a full
//! index; subsequent queries stat/read requested files and named ancestors.
use super::*;

impl Index {
    /// Native name lookup also works before the first rollout is created.
    pub fn codex_name(&self, sid: &str) -> Result<Option<String>, SessionError> {
        let Some(path) = &self.codex_index else {
            return Ok(None);
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| SessionError::new(500, "会话索引锁不可用"))?;
        let names = names::load(path, state.title_names.as_ref().or(state.names.as_ref()))?;
        let result = names.name(sid);
        state.title_names = Some(names);
        Ok(result)
    }

    pub fn titles(&self, ids: &[String]) -> Result<Value, SessionError> {
        if ids.is_empty() {
            return Ok(json!({"sessions": [], "index_initialized": false, "summary_reads": 0}));
        }
        let previous = self
            .state
            .lock()
            .map_err(|_| SessionError::new(500, "会话索引锁不可用"))?
            .snapshot
            .clone();
        let initialized = previous.is_none();
        let snapshot = match previous {
            Some(snapshot) => snapshot,
            None => self.refresh(false)?,
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| SessionError::new(500, "会话索引锁不可用"))?;
        // Keep this name snapshot separate from the list's invalidation state.
        let names = match &self.codex_index {
            Some(path) => Some(names::load(
                path,
                state.title_names.as_ref().or(state.names.as_ref()),
            )?),
            None => None,
        };
        state.title_names = names.clone();
        let mut entries = BTreeMap::new();
        let mut requested = BTreeMap::new();
        let mut missing = Vec::new();
        let mut reads = 0;
        for id in ids {
            let Some((source, sid)) = id.split_once(':') else {
                missing.push(id.clone());
                continue;
            };
            let Some(uids) = snapshot.threads.get(&(source.to_owned(), sid.to_owned())) else {
                missing.push(id.clone());
                continue;
            };
            let selected = if uids.len() == 1 {
                Some(uids[0].clone())
            } else {
                graph::generations(uids.iter().map(|uid| &snapshot.candidates[uid]).collect())
                    .and_then(|entries| entries.last().map(|entry| entry.uid.clone()))
            };
            let Some(mut uid) = selected else {
                missing.push(id.clone());
                continue;
            };
            requested.insert(id.clone(), uid.clone());
            while !entries.contains_key(&uid) {
                let Some(original) = snapshot.candidate(&uid) else {
                    break;
                };
                let mut entry = original.clone();
                let candidate = Discovered {
                    source: entry.source,
                    root: entry.root.clone(),
                    path: entry.path.clone(),
                    data: entry.data.clone(),
                    summary_path: entry.summary_path.clone(),
                    stamp: file_metadata(&entry.data).ok().map(|m| Stamp::of(&m)),
                    summary_stamp: entry
                        .summary_path
                        .as_ref()
                        .and_then(|p| file_metadata(p).ok())
                        .map(|m| Stamp::of(&m)),
                    agent_id: entry.agent_id.clone(),
                };
                if candidate.stamp.is_none() && !matches!(entry.source, "grok" | "opencode" | "agy")
                {
                    break;
                }
                let cached = state
                    .title_cache
                    .get(&entry.path)
                    .or_else(|| state.cache.get(&entry.path));
                entry.summary = if let Some(cached) = cached.filter(|c| c.key == candidate.key()) {
                    cached.summary.clone()
                } else {
                    reads += 1;
                    let Some(outcome) = read_candidate(
                        &candidate,
                        cached.and_then(|cached| cached.native_state.as_ref()),
                    ) else {
                        break;
                    };
                    if !outcome.transient {
                        state.title_cache.insert(
                            entry.path.clone(),
                            Cached {
                                key: outcome.key,
                                summary: outcome.summary.clone(),
                                native_state: outcome.native_state,
                            },
                        );
                    }
                    outcome.summary
                };
                let parent = entry
                    .summary
                    .codex
                    .as_ref()
                    .map(|c| &c.forked_from_id)
                    .and_then(|sid| snapshot.threads.get(&("codex".to_owned(), sid.clone())))
                    .filter(|uids| uids.len() == 1)
                    .map(|uids| uids[0].clone());
                entries.insert(uid, entry);
                match parent {
                    Some(parent) => uid = parent,
                    None => break,
                }
            }
        }
        let mains = graph::mains_by_sid(&entries);
        let mut rows = Vec::new();
        for (id, uid) in requested {
            let Some(entry) = entries.get(&uid) else {
                missing.push(id);
                continue;
            };
            let chain = graph::lineage(entry, &mains);
            let title = chain
                .last()
                .map(|root| &root.summary.title)
                .unwrap_or(&entry.summary.title);
            rows.push(json!({"session_uid":id, "uid":uid, "source":entry.source, "sid":entry.summary.sid, "title":title}));
        }
        if let Some(names) = names {
            names.enrich(&mut rows, &entries);
        }
        // No transcript paths, content, graph or other sessions in the response.
        for row in &mut rows {
            row.as_object_mut()
                .unwrap()
                .retain(|key, _| ["session_uid", "source", "sid", "title"].contains(&key.as_str()));
        }
        Ok(
            json!({"sessions":rows, "missing":missing, "index_initialized":initialized, "summary_reads":reads}),
        )
    }
}
