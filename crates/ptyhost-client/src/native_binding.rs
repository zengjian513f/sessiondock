//! One-time host association declared by a trusted operator, not native CLI proof.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::timeout;

use crate::{
    Error, HostClient, LaunchState, LaunchTarget, Result, Source,
    association::{Metadata, identifier},
    check_ok, wire,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NativeBinding {
    source: Source,
    sid: String,
    uid: String,
    launch_id: String,
    instance_id: String,
}
impl NativeBinding {
    pub fn source(&self) -> Source {
        self.source
    }
    pub fn sid(&self) -> &str {
        &self.sid
    }
    pub fn uid(&self) -> &str {
        &self.uid
    }
    pub fn launch_id(&self) -> &str {
        &self.launch_id
    }
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
    pub fn method(&self) -> &'static str {
        "operator"
    }
    fn parse(value: &Value, meta: &Metadata) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireBinding {
            version: u8,
            source: String,
            sid: String,
            uid: String,
            launch_id: String,
            instance_id: String,
            method: String,
        }
        let raw: WireBinding =
            serde_json::from_value(value.clone()).map_err(|_| Error::InvalidNativeBinding)?;
        let LaunchState::Declared(launch) = &meta.launch else {
            return Err(Error::InvalidNativeBinding);
        };
        if raw.version != 1
            || raw.method != "operator"
            || raw.source != launch.source.as_str()
            || raw.launch_id != launch.launch_id
            || Some(&raw.instance_id) != meta.instance_id.as_ref()
            || meta.native_identity_present
            || !valid_native(launch.source, &raw.sid, &raw.uid)
        {
            return Err(Error::InvalidNativeBinding);
        }
        Ok(Self {
            source: launch.source,
            sid: raw.sid,
            uid: raw.uid,
            launch_id: raw.launch_id,
            instance_id: raw.instance_id,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "status", content = "binding", rename_all = "snake_case")]
pub enum NativeBindingState {
    #[default]
    Unsupported,
    Unbound,
    Invalid,
    Bound(NativeBinding),
}
pub(crate) fn valid_native(source: Source, sid: &str, uid: &str) -> bool {
    source != Source::Shell
        && identifier(sid, 256)
        && identifier(uid, 256)
        && uid.starts_with(&format!("{}:", source.as_str()))
        && uid.len() > source.as_str().len() + 1
}
pub(crate) fn observe(value: &Value, meta: &Metadata) -> NativeBindingState {
    let capability = value.get("capabilities").and_then(|v| v.get("launch_bind"));
    if capability.is_none() && value.get("native_binding").is_none() {
        return NativeBindingState::Unsupported;
    }
    if capability.and_then(Value::as_u64) != Some(1) {
        return NativeBindingState::Invalid;
    }
    match value.get("native_binding") {
        Some(Value::Null) => NativeBindingState::Unbound,
        Some(binding) => NativeBinding::parse(binding, meta)
            .map(NativeBindingState::Bound)
            .unwrap_or(NativeBindingState::Invalid),
        None => NativeBindingState::Invalid,
    }
}

impl HostClient {
    /// Explicit same-tuple retries are idempotent, but this call never retries.
    /// A lost ACK is reconciled using guarded status, not a guessed association.
    pub async fn bind_launch(
        &self,
        target: &LaunchTarget,
        sid: &str,
        uid: &str,
    ) -> Result<NativeBinding> {
        if !valid_native(target.source(), sid, uid) {
            return Err(Error::InvalidNativeBinding);
        }
        timeout(self.limits.operation_timeout, async {
            let status = self.status_launch(target.name(),target.source(),target.launch_id(),target.instance_id()).await?;
            match &status.native_binding {
                NativeBindingState::Unsupported => return Err(Error::NativeBindingUnsupported),
                NativeBindingState::Invalid => return Err(Error::InvalidNativeBinding),
                NativeBindingState::Bound(binding) if binding.sid()!=sid || binding.uid()!=uid => return Err(Error::NativeBindingConflict),
                NativeBindingState::Unbound if status.exited => return Err(Error::InvalidNativeBinding),
                _ => {},
            }
            let record = self.read_record(target.name()).await?.ok_or(Error::NotFound)?;
            if !target.matches_record(&record) { return Err(Error::IdentityChanged); }
            let body = json!({"op":"launch_bind_v1","token":record.token.as_deref().unwrap_or_default(),
                "expected_instance_id":target.instance_id(),"expected_source":target.source(),
                "expected_launch_id":target.launch_id(),"native":{"sid":sid,"uid":uid}});
            let mut stream = self.connect(&record).await?;
            wire::send_json(&mut stream,&body,self.limits.max_line_bytes).await?;
            let reply = wire::read_json(&mut stream,&mut Vec::new(),self.limits.max_line_bytes).await?;
            if reply["ok"] == false && reply["code"] == "native_binding_conflict" { return Err(Error::NativeBindingConflict); }
            check_ok(&reply)?;
            target.check_ack(&reply)?;
            let binding = NativeBinding::parse(&reply["native_binding"],&record.meta)?;
            if binding.sid()!=sid || binding.uid()!=uid { return Err(Error::InvalidNativeBinding); }
            let after=self.read_record(target.name()).await?.ok_or(Error::IdentityChanged)?;
            if !record.same_identity(&after) { return Err(Error::IdentityChanged); }
            Ok(binding)
        }).await.map_err(|_|Error::Timeout)?
    }
}
