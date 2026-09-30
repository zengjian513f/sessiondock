//! Selected native rows for a same-store Codex clone. Schema is discovered and
//! retained with the plan; no native schema is created or upgraded by SessionDock.
use super::{
    TransferError,
    codex::{ClonePlan, StagedClone},
    codex_ids::{self, Identity},
};
use rusqlite::{
    Connection, OpenFlags,
    types::{Value as SqlValue, ValueRef},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

type Row = BTreeMap<String, Value>;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<String>,
    pub keys: Vec<String>,
    pub schema: String,
    pub rows: Vec<Row>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Database {
    pub path: PathBuf,
    pub tables: Vec<Table>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Native {
    pub databases: Vec<Database>,
}
impl From<rusqlite::Error> for TransferError {
    fn from(e: rusqlite::Error) -> Self {
        Self::new("move_native_database", e.to_string())
    }
}
fn value(v: ValueRef<'_>) -> Result<Value, TransferError> {
    Ok(match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(v) => v.into(),
        ValueRef::Real(v) => v.into(),
        ValueRef::Text(v) => String::from_utf8(v.to_vec())
            .map_err(|_| TransferError::new("move_native_unsupported", "原生元数据不是 UTF-8"))?
            .into(),
        ValueRef::Blob(_) => {
            return Err(TransferError::new(
                "move_native_unsupported",
                "原生元数据包含尚未适配的二进制字段",
            ));
        }
    })
}
fn sql(v: &Value) -> SqlValue {
    match v {
        Value::Null => SqlValue::Null,
        Value::String(s) => SqlValue::Text(s.clone()),
        Value::Number(n) if n.is_i64() => SqlValue::Integer(n.as_i64().unwrap()),
        Value::Number(n) => SqlValue::Real(n.as_f64().unwrap()),
        _ => unreachable!("native SQL scalar"),
    }
}
fn quote(v: &str) -> String {
    format!("\"{}\"", v.replace('"', "\"\""))
}
fn read(
    db: &Connection,
    name: &str,
    ids: &BTreeSet<String>,
) -> Result<Option<Table>, TransferError> {
    let schema = db.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name=?",
        [name],
        |r| r.get::<_, String>(0),
    );
    let schema = match schema {
        Ok(s) => s,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut info = db.prepare(&format!("PRAGMA table_info({})", quote(name)))?;
    let columns = info
        .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(5)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let keys = columns
        .iter()
        .filter(|(_, k)| *k > 0)
        .map(|(k, _)| k.clone())
        .collect();
    let columns: Vec<_> = columns.into_iter().map(|(k, _)| k).collect();
    let identity_columns: Vec<_> = if matches!(name, "threads" | "projects" | "thread_sections") {
        vec!["id"]
    } else if name == "project_roots" {
        vec!["project_id"]
    } else {
        ["thread_id", "parent_thread_id", "child_thread_id"]
            .into_iter()
            .filter(|k| columns.iter().any(|c| c == k))
            .collect()
    };
    if identity_columns.is_empty() {
        return Err(TransferError::new(
            "move_native_unsupported",
            format!("无法识别 {name} 的所属线程"),
        ));
    }
    let clause = identity_columns
        .iter()
        .map(|k| format!("{} IN (SELECT value FROM json_each(?1))", quote(k)))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut statement = db.prepare(&format!("SELECT * FROM {} WHERE {clause}", quote(name)))?;
    let encoded = serde_json::to_string(ids)?;
    let mut cursor = statement.query([encoded])?;
    let mut rows = Vec::new();
    while let Some(row) = cursor.next()? {
        let mut result = Row::new();
        for (i, k) in columns.iter().enumerate() {
            result.insert(k.clone(), value(row.get_ref(i)?)?);
        }
        rows.push(result);
    }
    rows.sort_by_cached_key(|r| serde_json::to_string(r).unwrap());
    Ok(Some(Table {
        name: name.into(),
        columns,
        keys,
        schema,
        rows,
    }))
}
fn owned(row: &Row, ids: &BTreeSet<String>) -> bool {
    ["thread_id", "parent_thread_id", "child_thread_id"]
        .iter()
        .any(|k| {
            row.get(*k)
                .and_then(Value::as_str)
                .is_some_and(|s| ids.contains(s))
        })
}
/// Read native data from explicit homes only. Related persistent stores that
/// need a separate adapter are surfaced, never silently discarded.
pub fn capture(home: &Path, plan: &ClonePlan) -> Result<Native, TransferError> {
    let ids: BTreeSet<_> = plan.identities.threads.keys().cloned().collect();
    let mut files = std::fs::read_dir(home)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|v| v == "sqlite"))
        .collect::<Vec<_>>();
    files.sort();
    let mut result = Native::default();
    let mut found_state = false;
    let mut found_history = false;
    for path in files {
        let filename = path.file_name().unwrap().to_string_lossy();
        let names: &[&str] = if filename.starts_with("state_") {
            found_state = true;
            &[
                "threads",
                "thread_dynamic_tools",
                "thread_spawn_edges",
                "thread_attachments",
                "thread_goals",
                "thread_goal_continuation_deferrals",
            ]
        } else if filename.starts_with("thread_history_") {
            found_history = true;
            &[
                "thread_turns",
                "thread_items",
                "thread_history_projection_state",
                "thread_realtime_items",
            ]
        } else if filename.starts_with("goals_") {
            &["thread_goals", "thread_goal_continuation_deferrals"]
        } else {
            continue;
        };
        let mut db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let tx = db.transaction()?;
        let mut tables = Vec::new();
        for name in names {
            let Some(mut table) = read(&tx, name, &ids)? else {
                continue;
            };
            table.rows.retain(|r| {
                if *name == "threads" {
                    r.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| ids.contains(id))
                } else {
                    owned(r, &ids)
                }
            });
            if !table.rows.is_empty()
                && matches!(*name, "thread_attachments" | "thread_realtime_items")
            {
                return Err(TransferError::new(
                    "move_native_unsupported",
                    format!("本组包含尚未适配的原生记录 {name}"),
                ));
            }
            if *name == "thread_spawn_edges"
                && table.rows.iter().any(|r| {
                    ["parent_thread_id", "child_thread_id"]
                        .iter()
                        .any(|k| !ids.contains(r[*k].as_str().unwrap_or("")))
                })
            {
                return Err(TransferError::new(
                    "move_group_incomplete",
                    "原生子代理图存在组外成员",
                ));
            }
            tables.push(table);
        }
        // Shared objects retain their IDs. Capture only associations required by
        // the selected threads, so another node can import missing objects.
        let mut shared = Vec::new();
        for (column, names) in [
            ("project_id", &["projects", "project_roots"][..]),
            ("thread_section_id", &["thread_sections"][..]),
        ] {
            let required: BTreeSet<_> = tables
                .iter()
                .filter(|t| t.name == "threads")
                .flat_map(|t| &t.rows)
                .filter_map(|r| r.get(column).and_then(Value::as_str))
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect();
            if required.is_empty() {
                continue;
            }
            for name in names {
                let table = read(&tx, name, &required)?;
                if *name != "project_roots" {
                    let found: BTreeSet<_> = table
                        .as_ref()
                        .into_iter()
                        .flat_map(|t| &t.rows)
                        .filter_map(|r| r.get("id").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect();
                    if found != required {
                        return Err(TransferError::new(
                            "move_group_incomplete",
                            "原生项目或分组关联缺失",
                        ));
                    }
                }
                if let Some(table) = table {
                    shared.push(table);
                }
            }
        }
        shared.extend(tables);
        let tables = shared;
        tx.commit()?;
        result.databases.push(Database { path, tables });
    }
    if plan.files.iter().any(|f| f.base.is_some()) && (!found_state || !found_history) {
        return Err(TransferError::new(
            "move_native_unsupported",
            "分页历史需要原生状态库及历史投影，当前未找到完整数据库",
        ));
    }
    Ok(result)
}
fn mapped(row: &mut Row, field: &str, ids: &BTreeMap<String, String>) -> Result<(), TransferError> {
    if let Some(Value::String(id)) = row.get_mut(field) {
        if !id.is_empty() {
            *id = ids.get(id).cloned().ok_or_else(|| {
                TransferError::new(
                    "move_group_incomplete",
                    format!("原生数据库未映射的 {field}"),
                )
            })?;
        }
    }
    Ok(())
}
fn boundary(
    plan: &ClonePlan,
    staged: &StagedClone,
    sid: &str,
    offset: u64,
    ordinal: u64,
    end: bool,
) -> Result<u64, TransferError> {
    if offset == 0 {
        return Ok(0);
    }
    let mut values = BTreeSet::new();
    for file in plan.files.iter().filter(|f| f.thread == sid) {
        let Some(output) = staged.files.iter().find(|f| f.source == file.source) else {
            continue;
        };
        let Some(next) = output.boundaries.get(&offset) else {
            continue;
        };
        let raw = std::fs::read(&file.source)?;
        let Some(bytes) = (if end {
            raw.get(..offset as usize)
        } else {
            raw.get(offset as usize..)
        }) else {
            continue;
        };
        let mut lines = bytes.split(|c| *c == b'\n').filter(|s| !s.is_empty());
        let line = if end { lines.next_back() } else { lines.next() };
        if let Some(line) = line {
            let v: Value = serde_json::from_slice(line)?;
            if v["ordinal"].as_u64() == Some(ordinal) {
                values.insert(*next);
            }
        }
    }
    if values.len() != 1 {
        return Err(TransferError::new(
            "move_native_unsupported",
            "无法唯一定位原生数据库游标对应的物理历史",
        ));
    }
    Ok(*values.first().unwrap())
}

pub fn rewrite(
    source: &Native,
    plan: &ClonePlan,
    staged: &StagedClone,
    home: &Path,
) -> Result<Native, TransferError> {
    if plan.mode == super::codex::Mode::Move {
        // Identity-preserving copies keep serialized projections and byte
        // cursors exactly as captured, including the current rollout pointer.
        return Ok(source.clone());
    }
    let mut result = source.clone();
    let map = &plan.identities;
    for db in &mut result.databases {
        for table in &mut db.tables {
            for row in &mut table.rows {
                let sid = row
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                for key in ["thread_id", "parent_thread_id", "child_thread_id"] {
                    mapped(row, key, &map.threads)?;
                }
                mapped(row, "turn_id", &map.turns)?;
                if table.name == "thread_goals" {
                    mapped(row, "goal_id", &map.records)?;
                }
                for key in ["item_id", "first_user_item_id", "final_agent_item_id"] {
                    mapped(row, key, &map.records)?;
                }
                if table.name == "threads" {
                    mapped(row, "id", &map.threads)?;
                    let path = row
                        .get("rollout_path")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let file = staged
                        .files
                        .iter()
                        .find(|f| f.source == Path::new(path))
                        .ok_or_else(|| {
                            TransferError::new(
                                "move_group_incomplete",
                                "当前 rollout 不在文件清单中",
                            )
                        })?;
                    row.insert(
                        "rollout_path".into(),
                        home.join(&file.relative)
                            .to_string_lossy()
                            .to_string()
                            .into(),
                    );
                    if let Some(Value::String(source)) = row.get_mut("source") {
                        if let Ok(mut v) = serde_json::from_str::<Value>(source) {
                            for spelling in ["subagent", "subAgent"] {
                                if let Some(id) = v.pointer_mut(&format!(
                                    "/{spelling}/thread_spawn/parent_thread_id"
                                )) {
                                    if let Some(old) = id.as_str() {
                                        *id = map
                                            .threads
                                            .get(old)
                                            .ok_or_else(|| {
                                                TransferError::new(
                                                    "move_group_incomplete",
                                                    "数据库子代理父身份未映射",
                                                )
                                            })?
                                            .clone()
                                            .into();
                                    }
                                }
                            }
                            *source = serde_json::to_string(&v)?;
                        }
                    }
                }
                if table.name == "thread_items" {
                    let text = row
                        .get("item_json")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            TransferError::new("move_native_unsupported", "原生 item_json 缺失")
                        })?;
                    let mut item: Value = serde_json::from_str(text)?;
                    codex_ids::item(&mut item, &mut |kind, id| {
                        let ids = match kind {
                            Identity::Thread => &map.threads,
                            Identity::Turn => &map.turns,
                            Identity::Record => &map.records,
                        };
                        ids.get(id).cloned().ok_or_else(|| {
                            TransferError::new(
                                "move_native_unsupported",
                                format!("原生投影身份未映射: {id}"),
                            )
                        })
                    })?;
                    row.insert("item_json".into(), serde_json::to_string(&item)?.into());
                }
                if table.name == "thread_turns" {
                    for (byte, ord, end) in [
                        ("rollout_byte_offset", "rollout_ordinal", false),
                        ("rollout_end_byte_offset", "rollout_end_ordinal", true),
                    ] {
                        if let (Some(b), Some(o)) = (
                            row.get(byte).and_then(Value::as_u64),
                            row.get(ord).and_then(Value::as_u64),
                        ) {
                            row.insert(
                                byte.into(),
                                boundary(plan, staged, &sid, b, o, end)?.into(),
                            );
                        }
                    }
                }
                if table.name == "thread_history_projection_state" {
                    let b = row["next_rollout_byte_offset"].as_u64().unwrap();
                    let o = row["next_rollout_ordinal"].as_u64().unwrap();
                    row.insert(
                        "next_rollout_byte_offset".into(),
                        boundary(plan, staged, &sid, b, o.saturating_sub(1), true)?.into(),
                    );
                }
            }
        }
    }
    Ok(result)
}

/// Verify schema and all target primary keys before any file is published.
pub fn preflight(native: &Native) -> Result<(), TransferError> {
    preflight_copy(native, false)
}
pub fn preflight_copy(native: &Native, reuse: bool) -> Result<(), TransferError> {
    for database in &native.databases {
        let db = Connection::open_with_flags(&database.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        for table in &database.tables {
            let current = read(&db, &table.name, &row_ids(table))?
                .ok_or_else(|| TransferError::new("move_native_unsupported", "目标表不存在"))?;
            if current.schema != table.schema {
                return Err(TransferError::new(
                    "move_plan_stale",
                    "原生数据库结构已变化",
                ));
            }
            if reuse
                && !shared_table(&table.name)
                && current.rows.iter().any(|row| !table.rows.contains(row))
            {
                return Err(TransferError::new(
                    "move_conflict",
                    "目标原生历史或当前版本不同",
                ));
            }
            for row in &table.rows {
                if let Some(existing) = current
                    .rows
                    .iter()
                    .find(|r| table.keys.iter().all(|k| r.get(k) == row.get(k)))
                {
                    if (reuse || shared_table(&table.name)) && existing == row {
                        continue;
                    }
                    return Err(TransferError::new(
                        "move_conflict",
                        "新身份已存在于原生数据库",
                    ));
                }
            }
        }
    }
    Ok(())
}
fn shared_table(name: &str) -> bool {
    matches!(name, "projects" | "project_roots" | "thread_sections")
}

/// Capture the destination rows after file-level prefix and rollout checks.
/// Existing history keys must be a subset; changing the current rollout is
/// never inferred merely from a matching thread ID.
pub fn preflight_prefix(
    native: &Native,
    extended: &BTreeSet<String>,
) -> Result<Native, TransferError> {
    let mut before = native.clone();
    for database in &mut before.databases {
        let db = Connection::open_with_flags(&database.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        for table in &mut database.tables {
            let current = read(&db, &table.name, &row_ids(table))?
                .ok_or_else(|| TransferError::new("move_native_unsupported", "目标表不存在"))?;
            if current.schema != table.schema {
                return Err(TransferError::new(
                    "move_plan_stale",
                    "原生数据库结构已变化",
                ));
            }
            for old in &current.rows {
                let next = table
                    .rows
                    .iter()
                    .find(|row| table.keys.iter().all(|k| row.get(k) == old.get(k)));
                let Some(next) = next else {
                    if shared_table(&table.name) {
                        continue;
                    }
                    return Err(TransferError::new(
                        "move_conflict",
                        "目标存在源端没有的原生历史",
                    ));
                };
                if old == next {
                    continue;
                }
                let eligible = if table.name == "threads" {
                    old.get("rollout_path")
                        .and_then(Value::as_str)
                        .is_some_and(|path| !path.is_empty())
                        && old.get("rollout_path") == next.get("rollout_path")
                        && old
                            .get("id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| extended.contains(id))
                } else {
                    !shared_table(&table.name) && owned(old, extended)
                };
                if !eligible {
                    return Err(TransferError::new(
                        "move_conflict",
                        "目标原生元数据或当前版本不同",
                    ));
                }
            }
            *table = current;
        }
    }
    Ok(before)
}

fn update_row(db: &Connection, table: &Table, row: &Row) -> Result<(), TransferError> {
    let values: Vec<_> = table
        .columns
        .iter()
        .chain(table.keys.iter())
        .map(|k| sql(&row[k]))
        .collect();
    let assignments = table
        .columns
        .iter()
        .map(|k| format!("{}=?", quote(k)))
        .collect::<Vec<_>>()
        .join(",");
    let clause = table
        .keys
        .iter()
        .map(|k| format!("{} IS ?", quote(k)))
        .collect::<Vec<_>>()
        .join(" AND ");
    db.execute(
        &format!(
            "UPDATE {} SET {assignments} WHERE {clause}",
            quote(&table.name)
        ),
        rusqlite::params_from_iter(values),
    )?;
    Ok(())
}
/// Retire only captured source rows; the operation journal retains their full
/// projection. Shared projects remain in place for unrelated source sessions.
pub fn retire(native: &Native, apply: bool) -> Result<(), TransferError> {
    for database in native.databases.iter().rev() {
        let mut db =
            Connection::open_with_flags(&database.path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys=ON")?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for table in database
            .tables
            .iter()
            .rev()
            .filter(|t| !shared_table(&t.name))
        {
            let current = read(&tx, &table.name, &row_ids(table))?
                .ok_or_else(|| TransferError::new("move_native_unsupported", "源端原生表不存在"))?;
            if current.schema != table.schema
                || current.rows.iter().any(|r| !table.rows.contains(r))
            {
                return Err(TransferError::new(
                    "move_plan_stale",
                    "源端原生记录已变化，保留清理现场",
                ));
            }
            if apply {
                for row in &current.rows {
                    let clause = table
                        .keys
                        .iter()
                        .map(|k| format!("{}=?", quote(k)))
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    tx.execute(
                        &format!("DELETE FROM {} WHERE {clause}", quote(&table.name)),
                        rusqlite::params_from_iter(table.keys.iter().map(|k| sql(&row[k]))),
                    )?;
                }
            }
        }
        tx.commit()?;
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    planned: Database,
    inserted: Database,
    #[serde(default)]
    replaced: Option<Database>,
}
/// Each database commits independently; the durable caller journal compensates
/// across databases. Inserts never replace another session's rows.
pub fn insert(native: &Native, operation: &str) -> Result<(), TransferError> {
    insert_copy(native, operation, false)
}
pub fn insert_copy(native: &Native, operation: &str, reuse: bool) -> Result<(), TransferError> {
    insert_with_prefix(native, operation, reuse, None)
}
pub fn insert_with_prefix(
    native: &Native,
    operation: &str,
    reuse: bool,
    before: Option<&Native>,
) -> Result<(), TransferError> {
    for database in &native.databases {
        let mut db =
            Connection::open_with_flags(&database.path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys=ON")?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch("CREATE TABLE IF NOT EXISTS _sessiondock_clone_journal (operation_id TEXT PRIMARY KEY, receipt TEXT NOT NULL)")?;
        let mut inserted = database.clone();
        for table in &mut inserted.tables {
            table.rows.clear();
        }
        let mut replaced = inserted.clone();
        for (table_index, table) in database.tables.iter().enumerate() {
            let current = read(&tx, &table.name, &row_ids(table))?
                .ok_or_else(|| TransferError::new("move_native_unsupported", "目标表不存在"))?;
            if current.schema != table.schema {
                return Err(TransferError::new(
                    "move_plan_stale",
                    "原生数据库结构已变化",
                ));
            }
            let approved = before
                .and_then(|n| n.databases.iter().find(|d| d.path == database.path))
                .and_then(|d| d.tables.iter().find(|t| t.name == table.name));
            if let Some(expected) = approved {
                if expected != &current {
                    return Err(TransferError::new(
                        "move_plan_stale",
                        "目标原生数据在发布前发生变化",
                    ));
                }
            }
            if reuse
                && approved.is_none()
                && !shared_table(&table.name)
                && current.rows.iter().any(|row| !table.rows.contains(row))
            {
                return Err(TransferError::new(
                    "move_conflict",
                    "目标原生历史或当前版本不同",
                ));
            }
            for row in &table.rows {
                if reuse || shared_table(&table.name) {
                    if let Some(existing) = current
                        .rows
                        .iter()
                        .find(|r| table.keys.iter().all(|k| r.get(k) == row.get(k)))
                    {
                        if existing == row {
                            continue;
                        }
                        if approved.is_some() && !shared_table(&table.name) {
                            update_row(&tx, table, row)?;
                            replaced.tables[table_index].rows.push(existing.clone());
                            continue;
                        }
                        return Err(TransferError::new(
                            "move_conflict",
                            "目标已有不同的项目或分组关联",
                        ));
                    }
                }
                let columns = table
                    .columns
                    .iter()
                    .map(|s| quote(s))
                    .collect::<Vec<_>>()
                    .join(",");
                let placeholders = table
                    .columns
                    .iter()
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(",");
                let values = table
                    .columns
                    .iter()
                    .map(|k| sql(&row[k]))
                    .collect::<Vec<_>>();
                tx.execute(
                    &format!(
                        "INSERT INTO {} ({columns}) VALUES ({placeholders})",
                        quote(&table.name)
                    ),
                    rusqlite::params_from_iter(values),
                )?;
                inserted.tables[table_index].rows.push(row.clone());
            }
        }
        tx.execute(
            "INSERT INTO _sessiondock_clone_journal VALUES (?,?)",
            [
                operation,
                &serde_json::to_string(&Receipt {
                    planned: database.clone(),
                    inserted,
                    replaced: Some(replaced),
                })?,
            ],
        )?;
        tx.commit()?;
    }
    Ok(())
}
/// Remove only this operation's exact inserted rows. A subsequently changed
/// row is retained and the operation stays blocked for recovery.
pub fn rollback(native: &Native, operation: &str) -> Result<(), TransferError> {
    for database in native.databases.iter().rev() {
        let mut db =
            Connection::open_with_flags(&database.path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        db.execute_batch("PRAGMA foreign_keys=ON")?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let exists:i64=tx.query_row("SELECT count(*) FROM sqlite_master WHERE name='_sessiondock_clone_journal' AND type='table'",[],|r|r.get(0))?;
        if exists == 0 {
            continue;
        }
        let receipt = tx.query_row(
            "SELECT receipt FROM _sessiondock_clone_journal WHERE operation_id=?",
            [operation],
            |r| r.get::<_, String>(0),
        );
        let (inserted, replaced) = match receipt {
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Ok(value) => {
                if let Ok(receipt) = serde_json::from_str::<Receipt>(&value) {
                    if receipt.planned != *database {
                        return Err(TransferError::new(
                            "move_recovery_required",
                            "数据库复制凭据不匹配",
                        ));
                    }
                    (receipt.inserted, receipt.replaced)
                } else {
                    // Journals written before shared-object imports recorded all
                    // rows as inserted. Keep recovery compatible with them.
                    let previous: Database = serde_json::from_str(&value)?;
                    if previous != *database {
                        return Err(TransferError::new(
                            "move_recovery_required",
                            "数据库复制凭据不匹配",
                        ));
                    }
                    (previous, None)
                }
            }
            Err(e) => return Err(e.into()),
        };
        for table in inserted.tables.iter().rev() {
            let restored = replaced
                .as_ref()
                .and_then(|d| d.tables.iter().find(|t| t.name == table.name));
            let mut ids = row_ids(table);
            if let Some(old) = restored {
                ids.extend(row_ids(old));
            }
            let current = read(&tx, &table.name, &ids)?
                .ok_or_else(|| TransferError::new("move_native_unsupported", "回滚所需表缺失"))?;
            if let Some(old) = restored {
                let planned = database
                    .tables
                    .iter()
                    .find(|t| t.name == table.name)
                    .unwrap();
                for original in &old.rows {
                    let same_key =
                        |r: &&Row| table.keys.iter().all(|k| r.get(k) == original.get(k));
                    let found = current.rows.iter().find(same_key);
                    let published = planned.rows.iter().find(same_key);
                    if found == Some(original) {
                        continue;
                    }
                    if found.is_none() || found != published {
                        return Err(TransferError::new(
                            "move_recovery_required",
                            "合并后的数据库记录已变化，保留现场",
                        ));
                    }
                    update_row(&tx, table, original)?;
                }
            }
            for row in &table.rows {
                let found = current
                    .rows
                    .iter()
                    .find(|r| table.keys.iter().all(|k| r.get(k) == row.get(k)));
                if let Some(found) = found {
                    if found != row {
                        return Err(TransferError::new(
                            "move_recovery_required",
                            "克隆数据库记录已变化，保留现场等待恢复",
                        ));
                    }
                    if shared_referenced(&tx, &table.name, row)? {
                        return Err(TransferError::new(
                            "move_recovery_required",
                            "本次导入的项目或分组已被其他记录引用，保留现场",
                        ));
                    }
                    let clause = table
                        .keys
                        .iter()
                        .map(|k| format!("{}=?", quote(k)))
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    let values = table.keys.iter().map(|k| sql(&row[k])).collect::<Vec<_>>();
                    tx.execute(
                        &format!("DELETE FROM {} WHERE {clause}", quote(&table.name)),
                        rusqlite::params_from_iter(values),
                    )?;
                }
            }
        }
        tx.execute(
            "DELETE FROM _sessiondock_clone_journal WHERE operation_id=?",
            [operation],
        )?;
        tx.commit()?;
    }
    Ok(())
}

fn shared_referenced(db: &Connection, name: &str, row: &Row) -> Result<bool, TransferError> {
    let references: &[(&str, &str)] = match name {
        "projects" => &[("threads", "project_id"), ("project_roots", "project_id")],
        "thread_sections" => &[("threads", "thread_section_id")],
        _ => return Ok(false),
    };
    for (table, column) in references {
        let exists: i64 = db.query_row(
            "SELECT count(*) FROM pragma_table_info(?1) WHERE name=?2",
            [table, column],
            |r| r.get(0),
        )?;
        if exists == 0 {
            continue;
        }
        let count: i64 = db.query_row(
            &format!(
                "SELECT count(*) FROM {} WHERE {}=?1",
                quote(table),
                quote(column)
            ),
            [sql(&row["id"])],
            |r| r.get(0),
        )?;
        if count != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Projection-only IDs (including code-mode collaboration calls) are not always
/// persisted as rollout events. Allocate them in the same confirmed map.
pub fn extend_identities(native: &Native, plan: &mut ClonePlan) -> Result<(), TransferError> {
    if plan.mode == super::codex::Mode::Move {
        return Ok(());
    }
    for db in &native.databases {
        for table in &db.tables {
            for row in &table.rows {
                for key in [
                    "item_id",
                    "first_user_item_id",
                    "final_agent_item_id",
                    "goal_id",
                ] {
                    if let Some(id) = row
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        if !plan.identities.records.contains_key(id) {
                            plan.identities
                                .records
                                .insert(id.into(), super::codex::uuid()?);
                        }
                    }
                }
                if let Some(id) = row.get("turn_id").and_then(Value::as_str) {
                    if !plan.identities.turns.contains_key(id) {
                        plan.identities
                            .turns
                            .insert(id.into(), super::codex::uuid()?);
                    }
                }
            }
        }
    }
    Ok(())
}

fn row_ids(table: &Table) -> BTreeSet<String> {
    table
        .rows
        .iter()
        .flat_map(|r| {
            [
                "id",
                "thread_id",
                "parent_thread_id",
                "child_thread_id",
                "project_id",
            ]
            .into_iter()
            .filter_map(|k| r.get(k).and_then(Value::as_str).map(str::to_owned))
        })
        .collect()
}
