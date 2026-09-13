//! Finite history pages. Grants hold checkpoints, never retained native views.
use super::{Event, MessageQuery, Selected, SessionError, ViewSnapshot, media_projection};
use crate::{files::FileService, media::MediaStore};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    sync::Mutex,
    time::{Duration, Instant},
};

const MAX_GRANTS: usize = 1024;
const GRANT_TTL: Duration = Duration::from_secs(10 * 60);
/// Events per page unless `PageStore::with_page_events` says otherwise
/// (batch 44 WP-A: `SESSIONDOCK_HISTORY_PAGE_EVENTS`, default 2000). The
/// 8 MiB JSON / 128-image / 24 MiB image targets below group a page. A
/// larger individual event gets its own page so history always advances.
pub const DEFAULT_PAGE_EVENTS: usize = 2000;
const MAX_IMAGES: usize = 128;
const MAX_IMAGE_BYTES: usize = 24 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
/// Typed images shown inline per message; the rest continue through media pages.
pub(super) const DISPLAY_LIMIT: usize = media_projection::DISPLAY_LIMIT;

#[derive(Clone)]
pub(crate) struct PageGrant {
    uid: String,
    agent: String,
    checkpoint: MessageQuery,
    next: usize,
    stop: usize,
    total: usize,
    issued: Instant,
}
/// Continuation of one message's typed images. Binds the exact event by its
/// non-status index, checkpoint and content identity; holds no payloads.
#[derive(Clone)]
pub(crate) struct MediaGrant {
    pub(super) uid: String,
    pub(super) agent: String,
    pub(super) checkpoint: MessageQuery,
    pub(super) index: usize,
    pub(super) identity: String,
    pub(super) offset: usize,
    pub(super) total: usize,
    pub(super) issued: Instant,
    /// Continuation token already handed out for this page, so repeated reads
    /// (retries, replays) return one stable `next` instead of minting grants.
    pub(super) next: Option<String>,
}
#[derive(Clone)]
enum Grant {
    Page(PageGrant),
    Media(MediaGrant),
}
impl Grant {
    fn issued(&self) -> Instant {
        match self {
            Self::Page(grant) => grant.issued,
            Self::Media(grant) => grant.issued,
        }
    }
    fn issued_mut(&mut self) -> &mut Instant {
        match self {
            Self::Page(grant) => &mut grant.issued,
            Self::Media(grant) => &mut grant.issued,
        }
    }
    fn scope(&self) -> (&str, &str) {
        match self {
            Self::Page(grant) => (&grant.uid, &grant.agent),
            Self::Media(grant) => (&grant.uid, &grant.agent),
        }
    }
}
struct Kind {
    malformed: &'static str,
    missing: &'static str,
    foreign: &'static str,
    expired: &'static str,
}
const PAGE_KIND: Kind = Kind {
    malformed: "历史页游标格式无效",
    missing: "历史页不存在或已淘汰，请重新载入会话",
    foreign: "历史页不属于所选会话或子代理",
    expired: "历史页已过期，请重新载入会话",
};
const MEDIA_KIND: Kind = Kind {
    malformed: "图片分页游标格式无效",
    missing: "图片分页不存在或已淘汰，请重新载入会话",
    foreign: "图片分页不属于所选会话或子代理",
    expired: "图片分页已过期，请重新载入会话",
};
pub struct PageStore {
    grants: Mutex<BTreeMap<String, Grant>>,
    page_events: usize,
}
impl Default for PageStore {
    fn default() -> Self {
        Self::with_page_events(DEFAULT_PAGE_EVENTS)
    }
}
impl PageStore {
    /// A store whose history pages select at most `page_events` events
    /// (clamped to 1..=10000), using soft byte and image targets.
    pub fn with_page_events(page_events: usize) -> Self {
        Self {
            grants: Mutex::new(BTreeMap::new()),
            page_events: page_events.clamp(1, 10_000),
        }
    }
    /// The configured event cap of one history page.
    pub fn page_events(&self) -> usize {
        self.page_events
    }
    fn issue(&self, mut grant: Grant) -> Result<String, SessionError> {
        let mut grants = self
            .grants
            .lock()
            .map_err(|_| SessionError::new(503, "历史分页暂不可用"))?;
        let now = Instant::now();
        grants.retain(|_, grant| now.duration_since(grant.issued()) < GRANT_TTL);
        while grants.len() >= MAX_GRANTS {
            let key = grants
                .iter()
                .min_by_key(|(_, grant)| grant.issued())
                .map(|(key, _)| key.clone())
                .expect("full cache");
            grants.remove(&key);
        }
        *grant.issued_mut() = now;
        for _ in 0..8 {
            let mut bytes = [0; 16];
            getrandom::fill(&mut bytes).map_err(|_| SessionError::new(503, "历史分页暂不可用"))?;
            let token = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if !grants.contains_key(&token) {
                grants.insert(token.clone(), grant);
                return Ok(token);
            }
        }
        Err(SessionError::new(503, "历史分页暂不可用"))
    }
    fn issue_page(&self, grant: PageGrant) -> Result<String, SessionError> {
        self.issue(Grant::Page(grant))
    }
    pub(super) fn issue_media(&self, grant: MediaGrant) -> Result<String, SessionError> {
        self.issue(Grant::Media(grant))
    }
    /// Remember `next` as the continuation of media grant `token`. Not a
    /// refresh: the TTL and content of `token` stay exactly as issued.
    fn link_media(&self, token: &str, next: &str) -> Result<(), SessionError> {
        let mut grants = self
            .grants
            .lock()
            .map_err(|_| SessionError::new(503, "历史分页暂不可用"))?;
        if let Some(Grant::Media(grant)) = grants.get_mut(token) {
            grant.next = Some(next.to_owned());
        }
        Ok(())
    }
    fn find(
        &self,
        token: &str,
        uid: &str,
        agent: &str,
        kind: &Kind,
    ) -> Result<Grant, SessionError> {
        if token.len() != 32
            || !token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(SessionError::new(400, kind.malformed));
        }
        let mut grants = self
            .grants
            .lock()
            .map_err(|_| SessionError::new(503, "历史分页暂不可用"))?;
        let grant = grants
            .get(token)
            .ok_or_else(|| SessionError::new(404, kind.missing))?;
        if grant.scope() != (uid, agent) {
            return Err(SessionError::new(403, kind.foreign));
        }
        if grant.issued().elapsed() >= GRANT_TTL {
            grants.remove(token);
            return Err(SessionError::new(410, kind.expired));
        }
        Ok(grant.clone())
    }
    pub(crate) fn lookup(
        &self,
        token: &str,
        uid: &str,
        agent: &str,
    ) -> Result<PageGrant, SessionError> {
        match self.find(token, uid, agent, &PAGE_KIND)? {
            Grant::Page(grant) => Ok(grant),
            Grant::Media(_) => Err(SessionError::new(404, PAGE_KIND.missing)),
        }
    }
    pub(crate) fn lookup_media(
        &self,
        token: &str,
        uid: &str,
        agent: &str,
    ) -> Result<MediaGrant, SessionError> {
        match self.find(token, uid, agent, &MEDIA_KIND)? {
            Grant::Media(grant) => Ok(grant),
            Grant::Page(_) => Err(SessionError::new(404, MEDIA_KIND.missing)),
        }
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        self.grants.lock().unwrap().len()
    }
    #[cfg(test)]
    fn age(&self, token: &str, by: Duration) {
        let mut grants = self.grants.lock().unwrap();
        let issued = grants.get_mut(token).unwrap().issued_mut();
        *issued = Instant::now() - by;
    }
}

struct Budget {
    events: usize,
    images: usize,
    image_bytes: usize,
    json_bytes: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            events: 0,
            images: 0,
            image_bytes: 0,
            json_bytes: 64 * 1024,
        }
    }
}
impl Budget {
    fn take(&mut self, event: &Event, limit: usize) -> Result<bool, SessionError> {
        if self.events >= limit {
            return Ok(false);
        }
        let mut counter = Counter(0);
        serde_json::to_writer(&mut counter, &event.message)
            .map_err(|_| SessionError::new(500, "历史消息序列化失败"))?;
        // Only the inline-displayed prefix is charged; the remainder is paged
        // separately through media grants and never enters this response.
        let displayed = event.media.len().min(DISPLAY_LIMIT);
        let image_bytes = event.media[..displayed]
            .iter()
            .filter(|image| image.file_ref().is_none())
            .map(|image| {
                if image.native_span().is_some() {
                    image.resident_len()
                } else {
                    image.encoded_len() / 4 * 3
                }
            })
            .sum::<usize>();
        let json_bytes = counter
            .0
            .saturating_add((displayed + crate::media::discover(&event.message).len()) * 8192)
            .saturating_add(32);
        if self.events > 0
            && (self.images.saturating_add(displayed) > MAX_IMAGES
                || self.image_bytes.saturating_add(image_bytes) > MAX_IMAGE_BYTES
                || self.json_bytes.saturating_add(json_bytes) > MAX_JSON_BYTES)
        {
            return Ok(false);
        }
        self.events += 1;
        self.images += displayed;
        self.image_bytes = self.image_bytes.saturating_add(image_bytes);
        self.json_bytes = self.json_bytes.saturating_add(json_bytes);
        Ok(true)
    }
}
struct Counter(usize);
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(super) fn validate_response(value: &Value) -> Result<(), SessionError> {
    serde_json::to_writer(Counter(0), value)
        .map_err(|_| SessionError::new(500, "历史页响应序列化失败"))
}

pub(super) fn window(
    snapshot: &ViewSnapshot,
    selected: &mut Vec<Selected<'_>>,
    pages: &PageStore,
) -> Result<Value, SessionError> {
    let total = selected.len();
    let mut budget = Budget::default();
    // Prioritize the latest tail. Heavy media may reduce either segment below
    // legacy's usual 100/500 events; all omitted events remain reachable.
    let mut stop = total;
    while stop > 0 && total - stop < 500 && budget.take(selected[stop - 1].event, 600)? {
        stop -= 1;
    }
    let mut start = 0;
    while start < stop && start < 100 && budget.take(selected[start].event, 600)? {
        start += 1;
    }
    if start == stop {
        return Ok(Value::Null);
    }
    let grant = PageGrant {
        uid: snapshot.view.meta["uid"].as_str().unwrap_or("").into(),
        agent: snapshot.view.meta["agent_id"].as_str().unwrap_or("").into(),
        checkpoint: snapshot.checkpoint(),
        next: start,
        stop,
        total,
        issued: Instant::now(),
    };
    let token = pages.issue_page(grant)?;
    selected.drain(start..stop);
    Ok(json!({"head":start,"tail":total-stop,"omitted":stop-start,"cursor":token}))
}

/// Content identity of one projected message, independent of media fields
/// (which are never part of `Event::message`) and of random descriptor tokens.
pub(super) fn event_identity(event: &Event) -> String {
    super::hash(&serde_json::to_vec(&event.message).expect("JSON values serialize"))
}

/// Continuation marker for a message whose typed images exceed the inline
/// limit. A grant is issued only when a page store is available; other callers
/// still report the remainder so no consumer mistakes 16 for the total.
pub(super) fn media_more(
    snapshot: &ViewSnapshot,
    selected: &Selected<'_>,
    pages: Option<&PageStore>,
) -> Result<Option<Value>, SessionError> {
    let total = selected.event.media.len();
    if total <= DISPLAY_LIMIT {
        return Ok(None);
    }
    let cursor = match pages {
        Some(pages) => Value::String(pages.issue_media(MediaGrant {
            uid: snapshot.view.meta["uid"].as_str().unwrap_or("").into(),
            agent: snapshot.view.meta["agent_id"].as_str().unwrap_or("").into(),
            checkpoint: snapshot.checkpoint(),
            index: selected.index,
            identity: event_identity(selected.event),
            offset: DISPLAY_LIMIT,
            total,
            issued: Instant::now(),
            next: None,
        })?),
        None => Value::Null,
    };
    Ok(Some(json!({
        "remaining": total - DISPLAY_LIMIT, "total": total, "cursor": cursor,
    })))
}

impl ViewSnapshot {
    /// The live checkpoint of this validated view, as grants record it.
    fn checkpoint(&self) -> MessageQuery {
        MessageQuery {
            start: self.view.parsed.committed as u64,
            head: self.head.clone(),
            anchor: self.anchor.clone(),
            ..Default::default()
        }
    }
    fn validate_grant_scope(
        &self,
        uid: &str,
        agent: &str,
        checkpoint: &MessageQuery,
    ) -> Result<(), SessionError> {
        if self.view.meta["uid"].as_str() != Some(uid)
            || self.view.meta["agent_id"].as_str().unwrap_or("") != agent
            || !self.valid_checkpoint(checkpoint)
        {
            return Err(SessionError::new(
                409,
                "历史页对应的时间线已变化，请重新载入会话",
            ));
        }
        Ok(())
    }
    /// Non-status events fixed by `checkpoint`; indexes are stable positions.
    fn checkpoint_events(&self, checkpoint: &MessageQuery) -> Vec<&Event> {
        self.view
            .events()
            .filter(|event| event.message["role"] != "status" && event.end <= checkpoint.start)
            .collect()
    }
    pub(crate) fn history_page(
        &self,
        grant: PageGrant,
        token: &str,
        media: &MediaStore,
        files: Option<&FileService>,
        pages: &PageStore,
    ) -> Result<Value, SessionError> {
        self.validate_grant_scope(&grant.uid, &grant.agent, &grant.checkpoint)?;
        let events = self.checkpoint_events(&grant.checkpoint);
        if events.len() != grant.total || grant.next >= grant.stop || grant.stop > events.len() {
            return Err(SessionError::new(409, "历史页范围已变化，请重新载入会话"));
        }
        let mut budget = Budget::default();
        let mut end = grant.next;
        while end < grant.stop && budget.take(events[end], pages.page_events())? {
            end += 1;
        }
        if end == grant.next {
            return Err(SessionError::new(413, "此历史页无法在读取预算内推进"));
        }
        let selected = events[grant.next..end]
            .iter()
            .enumerate()
            .map(|(offset, event)| Selected {
                index: grant.next + offset,
                event,
            })
            .collect::<Vec<_>>();
        let messages = super::project_selected(self, &selected, Some(media), files, Some(pages))?;
        let next = if end < grant.stop {
            let mut next = grant.clone();
            next.next = end;
            Some(pages.issue_page(next)?)
        } else {
            None
        };
        let response = json!({"messages":messages,"page":{"cursor":token,"next":next,"start":grant.next,"end":end,"stop":grant.stop,"remaining":grant.stop-end}});
        validate_response(&response)?;
        Ok(response)
    }

    /// One further batch of a single message's typed images. Descriptors are
    /// registered exactly as the inline projection registers them; the live
    /// cursor and the message list are never touched.
    pub(crate) fn media_page(
        &self,
        grant: MediaGrant,
        token: &str,
        media: &MediaStore,
        files: Option<&FileService>,
        pages: &PageStore,
    ) -> Result<Value, SessionError> {
        self.validate_grant_scope(&grant.uid, &grant.agent, &grant.checkpoint)
            .map_err(|_| SessionError::new(409, "图片分页对应的时间线已变化，请重新载入会话"))?;
        let events = self.checkpoint_events(&grant.checkpoint);
        let Some(event) = events.get(grant.index) else {
            return Err(SessionError::new(
                409,
                "图片分页对应的消息已不在时间线中，请重新载入会话",
            ));
        };
        if event_identity(event) != grant.identity || event.media.len() != grant.total {
            return Err(SessionError::new(
                409,
                "图片分页对应的消息已变化，请重新载入会话",
            ));
        }
        if grant.offset < DISPLAY_LIMIT || grant.offset >= grant.total {
            return Err(SessionError::new(409, "图片分页范围已变化，请重新载入会话"));
        }
        let end = (grant.offset + DISPLAY_LIMIT).min(grant.total);
        let items = media_projection::project_range(self, event, grant.offset..end, media, files)?;
        if items.len() != end - grant.offset {
            return Err(SessionError::new(413, "此图片分页无法在读取预算内推进"));
        }
        let next = if end < grant.total {
            // Reuse the continuation issued by an earlier read of this page
            // while it is still present, valid and describes the same range.
            let linked = grant.next.as_deref().filter(|next| {
                pages
                    .lookup_media(next, &grant.uid, &grant.agent)
                    .is_ok_and(|linked| {
                        linked.offset == end
                            && linked.total == grant.total
                            && linked.index == grant.index
                            && linked.identity == grant.identity
                            && linked.checkpoint.start == grant.checkpoint.start
                            && linked.checkpoint.head == grant.checkpoint.head
                            && linked.checkpoint.anchor == grant.checkpoint.anchor
                    })
            });
            match linked {
                Some(next) => Some(next.to_owned()),
                None => {
                    let mut next = grant.clone();
                    next.offset = end;
                    next.next = None;
                    let issued = pages.issue_media(next)?;
                    pages.link_media(token, &issued)?;
                    Some(issued)
                }
            }
        } else {
            None
        };
        let response = json!({"media":items,"page":{"cursor":token,"next":next,"start":grant.offset,"end":end,"total":grant.total,"remaining":grant.total-end}});
        validate_response(&response)
            .map_err(|_| SessionError::new(413, "图片分页响应超过 8 MiB 预算"))?;
        Ok(response)
    }
}

#[cfg(test)]
mod tests;
