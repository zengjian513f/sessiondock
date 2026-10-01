//! Atomic comparison and restoration of display rows during history imports.
use super::{MetadataSnapshot, Row, validate_uid};
use crate::metadata::{MetadataError, MetadataStore};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

impl MetadataStore {
    pub(crate) fn transfer_prefix_rows(
        &self,
        incoming: &BTreeMap<String, Value>,
        originals: &BTreeMap<String, Value>,
        restore: bool,
    ) -> Result<Arc<MetadataSnapshot>, MetadataError> {
        self.update(|snapshot| {
            snapshot.change(|rows| {
                for (uid, value) in incoming {
                    validate_uid(uid)?;
                    let parse = |value: &Value| {
                        serde_json::from_value::<Row>(value.clone()).map_err(|_| {
                            MetadataError::new(409, "move_metadata_invalid", "复制元数据无效")
                        })
                    };
                    let after = parse(value)?;
                    let before = originals
                        .get(uid)
                        .map(parse)
                        .transpose()?
                        .unwrap_or_default();
                    if after == Row::default() && before == Row::default() {
                        continue;
                    }
                    let current = rows.get(uid).cloned().unwrap_or_default();
                    let (expected, replacement) = if restore {
                        (&after, &before)
                    } else {
                        (&before, &after)
                    };
                    // A crash can occur before publication or after compensation.
                    if &current == replacement {
                        continue;
                    }
                    if &current != expected {
                        return Err(MetadataError::new(
                            409,
                            if restore {
                                "move_recovery_required"
                            } else {
                                "move_conflict"
                            },
                            "目标会话显示设置已变化，保留现场",
                        ));
                    }
                    if replacement == &Row::default() {
                        rows.remove(uid);
                    } else {
                        rows.insert(uid.clone(), replacement.clone());
                    }
                }
                Ok(())
            })
        })
    }
}
