//! Claude native acknowledgment adapter.
//!
//! Turns one checked read of a Claude main session's committed `user` inputs
//! into the Claude Machine's evidence types. The only association it can
//! establish after a managed Enter is `VerifiedEnter`: the executor held the write lease,
//! verified the exact prepared composer, captured the physical fence before
//! any write and pressed Enter; a real human `user` record that begins after
//! that fence and carries exactly the delivered text (end-trimmed only) is
//! attributed to that Enter. A matching native input after an attempted paste
//! also confirms a manually submitted draft via `PossibleTextMatch`.
//! Nothing here reads a screen, a hook or a timer,
//! and nothing here can turn "no record yet" into failure or retry.

use super::claude::{
    Association, Cursor, NativeContext, NativeRecord, Receipt, Scope, Turn, UserEvidence,
};
use crate::sessions::{NativeFence, NativeInputRead};

/// Claude records the prompt without the editor's outer
/// whitespace; internal spaces and newlines stay significant.
pub fn prompt_key(text: &str) -> &str {
    text.trim()
}

pub fn cursor_of(fence: &NativeFence) -> Cursor {
    Cursor {
        source_identity: fence.source_identity.clone(),
        offset: fence.offset,
        head: fence.head.clone(),
        anchor: fence.anchor.clone(),
    }
}

pub fn fence_of(cursor: &Cursor) -> NativeFence {
    NativeFence {
        source_identity: cursor.source_identity.clone(),
        offset: cursor.offset,
        head: cursor.head.clone(),
        anchor: cursor.anchor.clone(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    /// A real user record after the fence carries the delivered text.
    Accepted(Box<UserEvidence>),
    /// No matching record yet; the watch cursor may advance to `next`.
    Nothing { next: Option<Cursor> },
    /// The fence is no longer a checkpoint of the session (rewrite, truncation
    /// or identity change). The receipt stays as it is; nothing is inferred.
    FenceInvalid,
}

/// Relate one checked read to a receipt that crossed the paste boundary.
pub fn observe(
    receipt: &Receipt,
    scope: &Scope,
    from: &Cursor,
    read: &NativeInputRead,
) -> Observation {
    if !read.fence_valid || read.current.source_identity != from.source_identity {
        return Observation::FenceInvalid;
    }
    let Some(confirmation) = &receipt.confirmation else {
        return Observation::Nothing { next: None };
    };
    if !receipt.attempted {
        return Observation::Nothing { next: None };
    }
    let wanted = prompt_key(&receipt.request.payload.text);
    let matched = read.inputs.iter().find(|input| {
        input.end > input.start
            && input.start >= confirmation.offset
            && input.start >= from.offset
            && !input.uuid.trim().is_empty()
            && prompt_key(&input.text) == wanted
    });
    if let Some(input) = matched {
        return Observation::Accepted(Box::new(UserEvidence {
            context: NativeContext {
                scope: scope.clone(),
                confirmation: confirmation.clone(),
                record: NativeRecord {
                    source_identity: read.current.source_identity.clone(),
                    id: input.uuid.clone(),
                    start: input.start,
                    end: input.end,
                },
            },
            text: input.text.clone(),
            attachments: Vec::new(),
            turn: Turn {
                user_uuid: input.uuid.clone(),
                parent_turn_uuid: None,
            },
            real_human_input: true,
            association: receipt
                .enter
                .clone()
                .map_or(Association::PossibleTextMatch, Association::VerifiedEnter),
        }));
    }
    let current = cursor_of(&read.current);
    let next = (current.offset > from.offset).then_some(current);
    Observation::Nothing { next }
}

#[cfg(test)]
mod tests;
