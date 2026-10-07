//! Codex native names read on demand, before the first rollout exists.
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
}
