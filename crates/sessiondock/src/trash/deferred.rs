//! Deferred parent deletion: manifests retain history until its last dependent
//! is permanently removed. All native moves reuse the recoverable trash path.
use super::*;

fn text<'a>(row: &'a Value, key: &str) -> &'a str {
    row[key].as_str().unwrap_or_default()
}
fn identity(row: &Value) -> (&str, &str) {
    (text(row, "source"), text(row, "sid"))
}
fn members(manifest: &Manifest) -> Vec<Value> {
    if manifest.sessions.is_empty() {
        vec![
            json!({"uid":manifest.uid,"source":manifest.source,"sid":manifest.sid,"path":manifest.origin,"title":manifest.title}),
        ]
    } else {
        manifest.sessions.clone()
    }
}
fn uids(rows: &[Value]) -> BTreeSet<String> {
    rows.iter().map(|row| text(row, "uid").to_owned()).collect()
}

fn native_member(row: &Value, rows: &[Value]) -> bool {
    Path::new(text(row, "path")).exists()
        && rows
            .iter()
            .any(|current| identity(current) == identity(row) && current["path"] == row["path"])
}

impl TrashService {
    pub fn pending_deletions(&self) -> Result<BTreeSet<(String, String)>, TrashError> {
        let _guard = self.guard();
        Ok(self
            .scan()?
            .0
            .into_iter()
            .filter_map(|(_, m)| m.ok())
            .filter(|m| m.deferred)
            .map(|m| (m.source, m.sid))
            .collect())
    }

    pub(super) fn dependency_rows(&self, rows: &[Value]) -> Result<Vec<Value>, TrashError> {
        let mut all = rows.to_vec();
        for (_, manifest) in self.scan()?.0 {
            let Ok(manifest) = manifest else {
                continue;
            };
            all.extend(members(&manifest));
        }
        let mut seen = BTreeSet::new();
        all.retain(|row| {
            seen.insert((
                text(row, "uid").to_owned(),
                text(row, "source").to_owned(),
                text(row, "sid").to_owned(),
            ))
        });
        Ok(all)
    }

    /// A logical thread owns every immutable rollout generation. A branch may
    /// name either the stable thread ID or a physical rollout ID as its base.
    pub(super) fn depends_on(child: &Value, parent: &Value) -> bool {
        if identity(child) == identity(parent) || text(child, "source") != text(parent, "source") {
            return false;
        }
        let sid = text(parent, "sid");
        if sid.is_empty() {
            return false;
        }
        let base = text(&child["history_base"], "thread_id");
        let fork = text(child, "forked_from_id");
        if (child["supported"] != false || child["history_base"].is_object()) && fork == sid {
            return true;
        }
        if base.is_empty() {
            return false;
        }
        base == sid
            || (text(parent, "source") == "codex"
                && crate::sessions::codex_rollout_id(Path::new(text(parent, "path")), sid)
                    == Some(base))
    }

    pub(super) fn descendants(rows: &[Value], roots: &[Value]) -> Vec<Value> {
        let mut scope = roots.to_vec();
        let mut seen = uids(roots);
        loop {
            let found = rows
                .iter()
                .filter(|row| {
                    !seen.contains(text(row, "uid"))
                        && scope.iter().any(|parent| {
                            identity(parent) == identity(row) || Self::depends_on(row, parent)
                        })
                })
                .cloned()
                .collect::<Vec<_>>();
            if found.is_empty() {
                break;
            }
            seen.extend(uids(&found));
            scope.extend(found);
        }
        let roots = uids(roots);
        scope
            .into_iter()
            .filter(|row| !roots.contains(text(row, "uid")))
            .collect()
    }

    pub(super) fn logical_plan(&self, row: &Value, rows: &[Value]) -> Result<Plan, TrashError> {
        let family = rows
            .iter()
            .filter(|other| identity(other) == identity(row))
            .cloned()
            .collect::<Vec<_>>();
        let mut plan = Plan::derive(row, &self.roots)?;
        self.extend_plan(&mut plan, &family)?;
        plan.sessions = family;
        Ok(plan)
    }

    fn extend_plan(&self, plan: &mut Plan, rows: &[Value]) -> Result<(), TrashError> {
        let mut paths = plan
            .files
            .iter()
            .map(|f| f.origin.clone())
            .collect::<BTreeSet<_>>();
        for row in rows {
            if !Path::new(text(row, "path")).exists() {
                continue;
            }
            for file in Plan::derive(row, &self.roots)?.files {
                if paths.insert(file.origin.clone()) {
                    plan.files.push(file);
                }
            }
        }
        Ok(())
    }

    fn cascade_scope(&self, manifest: &Manifest, rows: &[Value]) -> Result<Vec<Value>, TrashError> {
        let universe = self.dependency_rows(rows)?;
        let entries = self.scan()?.0;
        let mut scope = members(manifest);
        loop {
            let before = uids(&scope);
            scope.extend(Self::descendants(&universe, &scope));
            let selected = uids(&scope);
            // An existing whole-tree trash entry is indivisible; include its
            // complete membership in the preview before allowing its purge.
            for (_, m) in &entries {
                let Ok(m) = m.as_ref() else {
                    continue;
                };
                let family = members(m);
                if family.iter().any(|r| selected.contains(text(r, "uid"))) {
                    scope.extend(family);
                }
            }
            let mut seen = BTreeSet::new();
            scope.retain(|r| seen.insert(text(r, "uid").to_owned()));
            if seen == before {
                break;
            }
        }
        Ok(scope)
    }

    pub fn list_with_dependencies(
        &self,
        limit: usize,
        cursor: Option<&str>,
        rows: &[Value],
    ) -> Result<Listing, TrashError> {
        let _guard = self.guard();
        let mut listing = self.list_inner(limit, cursor)?;
        for item in &mut listing.items {
            if item["deferred"] != true {
                continue;
            }
            let m = Manifest::read(&self.entry_dir(text(item, "id"))?)?;
            let roots = uids(&members(&m));
            item["cascade_sessions"] = json!(
                self.cascade_scope(&m, rows)?
                    .into_iter()
                    .filter(|r| !roots.contains(text(r, "uid")))
                    .collect::<Vec<_>>()
            );
        }
        Ok(listing)
    }

    /// Stage all surviving native members before erasing any recovery entry.
    fn purge_cascade(
        &self,
        id: &str,
        scope: &[Value],
        rows: &[Value],
    ) -> Result<(usize, u64), TrashError> {
        let selected = uids(scope);
        let native = scope
            .iter()
            .filter(|row| native_member(row, rows))
            .cloned()
            .collect::<Vec<_>>();
        let staged = if let Some(first) = native.first() {
            let mut plan = Plan::derive(first, &self.roots)?;
            self.extend_plan(&mut plan, &native)?;
            plan.sessions = native;
            Some(
                self.move_into_trash(
                    &plan,
                    RunStateNote {
                        state: "exited".into(),
                        detail: "cascade".into(),
                    },
                    false,
                )?
                .entry_id,
            )
        } else {
            None
        };
        let entries = self.scan()?.0;
        let mut remove = Vec::new();
        for (entry_id, m) in entries {
            let Ok(m) = m else {
                continue;
            };
            if entry_id == id
                || members(&m)
                    .iter()
                    .any(|r| selected.contains(text(r, "uid")))
            {
                if m.deferred {
                    // All native moves succeeded. This marker no longer owns
                    // files; only then may it be removed.
                    fs::remove_dir_all(self.entry_dir(&entry_id)?)
                        .map_err(|_| TrashError::new(500, "purge_failed", "待删除标记无法清理"))?;
                } else {
                    remove.push(entry_id);
                }
            }
        }
        if let Some(staged) = staged
            && !remove.contains(&staged)
        {
            remove.push(staged);
        }
        let mut freed = 0;
        for entry in &remove {
            freed += self.purge_one(entry)?;
        }
        Ok((remove.len().max(1), freed))
    }

    pub fn purge_confirmed(
        &self,
        rows: &[Value],
        live: &Liveness,
        ids: &[String],
        days: Option<u64>,
        confirmed: &[String],
    ) -> Result<PurgeOutcome, TrashError> {
        let _guard = self.guard();
        let entries = self.scan()?.0;
        let candidates: Vec<_> = if let Some(days) = days {
            let cutoff = now_unix().saturating_sub(days.saturating_mul(86400));
            entries
                .iter()
                .rev()
                .filter(|(_, m)| {
                    m.as_ref()
                        .map_or(days == 0, |m| m.deleted_at_unix <= cutoff)
                })
                .map(|(id, _)| id.clone())
                .collect()
        } else {
            ids.to_vec()
        };
        let mut scopes = BTreeMap::new();
        let mut expected = BTreeSet::new();
        for id in &candidates {
            if let Some((_, Ok(m))) = entries.iter().find(|(entry, _)| entry == id)
                && m.deferred
            {
                let scope = self.cascade_scope(m, rows)?;
                expected.extend(uids(&scope));
                scopes.insert(id.clone(), scope);
            }
        }
        if !expected.is_empty() && expected != confirmed.iter().cloned().collect() {
            return Err(TrashError::new(
                409,
                "cascade_changed",
                "父子会话范围已变化，请刷新回收站并确认连同子会话彻底删除",
            ));
        }
        for scope in scopes.values() {
            for row in scope {
                if native_member(row, rows)
                    && matches!(live.state(text(row, "uid")), RunState::Running(_))
                {
                    return Err(TrashError::new(
                        409,
                        "session_running",
                        format!("子会话「{}」仍在运行，请先停止", text(row, "title")),
                    ));
                }
            }
        }
        let mut outcome = PurgeOutcome::default();
        for id in candidates {
            // An earlier parent may already have purged this child entry.
            if !self.entry_dir(&id)?.exists() && outcome.removed > 0 {
                continue;
            }
            let result = match scopes.get(&id) {
                Some(scope) => self.purge_cascade(&id, scope, rows),
                None => self.purge_one(&id).map(|bytes| (1, bytes)),
            };
            match result {
                Ok((count, bytes)) => {
                    outcome.removed += count;
                    outcome.freed += bytes;
                }
                Err(error) => {
                    outcome.errors.push(error.message.clone());
                    outcome
                        .failed
                        .push(json!({"id":id,"code":error.code,"error":error.message}));
                }
            }
        }
        // A child in the recycle bin still counts as a dependent. Only its
        // permanent removal permits collecting a pending ancestor.
        loop {
            let remaining = rows
                .iter()
                .filter(|r| Path::new(text(r, "path")).exists())
                .cloned()
                .collect::<Vec<_>>();
            let mut collected = false;
            for (id, m) in self.scan()?.0 {
                let Ok(m) = m else {
                    continue;
                };
                if !m.deferred {
                    continue;
                }
                let scope = self.cascade_scope(&m, &remaining)?;
                let roots = members(&m);
                if scope
                    .iter()
                    .any(|r| !roots.iter().any(|p| identity(r) == identity(p)))
                {
                    continue;
                }
                if scope.iter().any(|r| {
                    native_member(r, &remaining)
                        && matches!(live.state(text(r, "uid")), RunState::Running(_))
                }) {
                    continue;
                }
                match self.purge_cascade(&id, &scope, &remaining) {
                    Ok((count, bytes)) => {
                        outcome.removed += count;
                        outcome.freed += bytes;
                        collected = true;
                    }
                    Err(e) => {
                        outcome.errors.push(e.message.clone());
                        outcome
                            .failed
                            .push(json!({"id":id,"code":e.code,"error":e.message}));
                    }
                }
            }
            if !collected {
                break;
            }
        }
        outcome.remaining = self.scan()?.0.len();
        Ok(outcome)
    }
}
