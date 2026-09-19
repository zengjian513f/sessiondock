use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::timeout;

use crate::AttachMode;
use crate::{
    Association, AssociationState, Attachment, ControlOp, ControlReply, Error, HostClient,
    HostObservation, Result, Source, TerminalSize,
    association::identifier,
    check_ok,
    dto::{HostRecord, validate_name},
    parse_reply, wire,
};

/// Native-catalog identity bound to an explicitly declared host instance.
/// No host credentials are held here. This type is not a browser lease and is
/// intentionally not serializable. The caller must first resolve a unique,
/// supported main-session row in its frozen catalog.
#[derive(Clone)]
pub struct BoundTarget {
    name: String,
    source: Source,
    sid: String,
    uid: String,
    instance_id: String,
    declared: Association,
    origin: Option<crate::NativeBinding>,
}

impl BoundTarget {
    pub fn from_observation(
        observation: &HostObservation,
        source: Source,
        full_sid: &str,
        full_uid: &str,
    ) -> Result<Self> {
        if !observation.instance_guard_v1 {
            return Err(Error::GuardUnsupported);
        }
        let Some(instance_id) = observation
            .instance_id
            .as_ref()
            .filter(|id| identifier(id, 128) && id.len() >= 16)
        else {
            return Err(Error::InvalidBinding);
        };
        let (declared, origin) = match &observation.native_binding {
            crate::NativeBindingState::Bound(binding) => {
                if binding.source() != source
                    || binding.sid() != full_sid
                    || binding.uid() != full_uid
                    || binding.instance_id() != instance_id
                    || !matches!(&observation.launch,crate::LaunchState::Declared(launch) if launch.source==source && launch.launch_id==binding.launch_id())
                {
                    return Err(Error::InvalidBinding);
                }
                (
                    Association {
                        source,
                        sid: Some(full_sid.into()),
                        uid: Some(full_uid.into()),
                    },
                    Some(binding.clone()),
                )
            }
            crate::NativeBindingState::Invalid => return Err(Error::InvalidNativeBinding),
            _ => {
                let AssociationState::Declared(declared) = &observation.association else {
                    return Err(Error::InvalidBinding);
                };
                (declared.clone(), None)
            }
        };
        if observation.exited
            || validate_name(&observation.summary.name).is_err()
            || !identifier(full_sid, 256)
            || !identifier(full_uid, 256)
            || !full_uid.starts_with(&format!("{}:", source.as_str()))
            || full_uid.len() == source.as_str().len() + 1
            || declared.source != source
            || (declared.sid.is_none() && declared.uid.is_none())
            || declared.sid.as_deref().is_some_and(|sid| sid != full_sid)
            || declared.uid.as_deref().is_some_and(|uid| uid != full_uid)
        {
            return Err(Error::InvalidBinding);
        }
        Ok(Self {
            name: observation.summary.name.clone(),
            source,
            sid: full_sid.into(),
            uid: full_uid.into(),
            instance_id: instance_id.clone(),
            declared,
            origin,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn source(&self) -> Source {
        self.source
    }
    pub fn sid(&self) -> &str {
        &self.sid
    }
    pub fn uid(&self) -> &str {
        &self.uid
    }
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
    pub fn origin_launch_id(&self) -> Option<&str> {
        self.origin.as_ref().map(|binding| binding.launch_id())
    }

    fn matches_record(&self, record: &HostRecord) -> bool {
        record.name == self.name
            && record.meta.instance_id.as_ref() == Some(&self.instance_id)
            && if let Some(origin) = &self.origin {
                !record.meta.native_identity_present
                    && matches!(&record.meta.launch,crate::LaunchState::Declared(launch) if launch.source==self.source && launch.launch_id==origin.launch_id())
            } else {
                record.meta.association == AssociationState::Declared(self.declared.clone())
            }
    }

    fn envelope(&self, token: Option<&str>, request: Value) -> Value {
        let mut value = json!({"op":"guarded_v1","token":token.unwrap_or_default(),
            "expected_instance_id":self.instance_id,"expected_source":self.source,"request":request});
        if let Some(sid) = &self.declared.sid {
            value["expected_sid"] = json!(sid);
        }
        if let Some(uid) = &self.declared.uid {
            value["expected_uid"] = json!(uid);
        }
        value
    }

    fn check_ack(&self, value: &Value) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct GuardAck {
            version: u8,
            instance_id: String,
        }
        let ack: GuardAck = serde_json::from_value(
            value
                .get("instance_guard")
                .cloned()
                .ok_or(Error::GuardNotAcknowledged)?,
        )
        .map_err(|_| Error::GuardNotAcknowledged)?;
        if ack.version != 1 || ack.instance_id != self.instance_id {
            return Err(Error::GuardNotAcknowledged);
        }
        Ok(())
    }
}

impl HostClient {
    /// The guarded envelope is a new operation, so a legacy host rejects it
    /// before interpreting the inner write. There is never an unguarded retry.
    /// Missing/wrong ack after submission is ambiguous and must not be retried.
    pub async fn request_bound(
        &self,
        target: &BoundTarget,
        operation: ControlOp,
    ) -> Result<ControlReply> {
        if let ControlOp::Rename { to } = &operation {
            validate_name(to)?;
        }
        timeout(self.limits.operation_timeout, async {
            let record = self
                .read_record(target.name())
                .await?
                .ok_or(Error::NotFound)?;
            if !target.matches_record(&record) {
                return Err(Error::IdentityChanged);
            }
            let mut stream = self.connect(&record).await?;
            let request = serde_json::to_value(&operation).map_err(|_| Error::InvalidRequest)?;
            let body = target.envelope(record.token.as_deref(), request);
            wire::send_json(&mut stream, &body, self.limits.max_line_bytes).await?;
            let reply =
                wire::read_json(&mut stream, &mut Vec::new(), self.limits.max_line_bytes).await?;
            check_ok(&reply)?;
            target.check_ack(&reply)?;
            parse_reply(operation, reply)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// After a guarded attach acknowledgement, the connected stream belongs to
    /// that immutable host instance. Later same-name endpoint replacement cannot
    /// redirect this existing stream. Browser ownership still belongs above us.
    pub async fn attach_bound(
        &self,
        target: &BoundTarget,
        size: TerminalSize,
        replay: bool,
    ) -> Result<Attachment> {
        self.attach_bound_mode(target, size, replay, AttachMode::Bytes)
            .await
    }

    /// `attach_bound` with an explicit stream mode (`Grid` streams JSON lines).
    pub async fn attach_bound_mode(
        &self,
        target: &BoundTarget,
        size: TerminalSize,
        replay: bool,
        mode: AttachMode,
    ) -> Result<Attachment> {
        timeout(self.limits.operation_timeout, async {
            let record = self
                .read_record(target.name())
                .await?
                .ok_or(Error::NotFound)?;
            if !target.matches_record(&record) {
                return Err(Error::IdentityChanged);
            }
            let mut stream = self.connect(&record).await?;
            let body = target.envelope(record.token.as_deref(), {
                let mut request =
                    json!({"op":"attach","cols":size.cols(),"rows":size.rows(),"replay":replay});
                if mode == AttachMode::Grid {
                    request["mode"] = json!("grid");
                }
                request
            });
            wire::send_json(&mut stream, &body, self.limits.max_line_bytes).await?;
            let mut buffer = Vec::new();
            let reply =
                wire::read_json(&mut stream, &mut buffer, self.limits.max_line_bytes).await?;
            check_ok(&reply)?;
            target.check_ack(&reply)?;
            self.attachment_from_reply(stream, buffer, reply)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}
