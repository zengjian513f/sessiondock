//! Native cumulative model totals and summary context have different scopes.
use super::*;
use crate::sdk_common::usage::Usage;

fn counters(live: &mut Live) -> Usage<'_> {
    Usage {
        usage: &mut live.usage,
        model_usage: &mut live.model_usage,
        message_usage: &mut live.message_usage,
    }
}

pub(super) fn result(live: &mut Live, frame: &Value) {
    counters(live).result(frame);
}

pub(super) fn assistant(live: &mut Live, id: &str, native: &Value) {
    counters(live).assistant(id, native);
}
