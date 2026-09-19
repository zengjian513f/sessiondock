//! Guarded access to one explicit launch instance, including pending sessions
//! without native SID/UID. This does not create a native BoundTarget or lease.

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::time::timeout;

use crate::AttachMode;
use crate::{
    Attachment, ControlOp, ControlReply, Error, HostClient, HostObservation, LaunchIdentity,
    LaunchState, Result, Source, TerminalSize,
    association::identifier,
    check_ok,
    dto::{HostRecord, validate_name},
    parse_reply, wire,
};

/// Caller-proven launch/instance nonces pinned to an observed host. No native
/// identity is inferred. Intentionally neither Debug nor Serialize; this is
/// internal control authority, not a browser lease or launch receipt.
#[derive(Clone)]
pub struct LaunchTarget {
    name: String,
    identity: LaunchIdentity,
    instance_id: String,
}

impl LaunchTarget {
    pub fn from_observation(
        observation: &HostObservation,
        source: Source,
        full_launch_id: &str,
        expected_instance_id: &str,
    ) -> Result<Self> {
        Self::checked_observation(
            observation,
            source,
            full_launch_id,
            expected_instance_id,
            false,
        )
    }

    fn checked_observation(
        observation: &HostObservation,
        source: Source,
        full_launch_id: &str,
        expected_instance_id: &str,
        allow_exited: bool,
    ) -> Result<Self> {
        if !observation.launch_guard_v1 {
            return Err(Error::LaunchGuardUnsupported);
        }
        let LaunchState::Declared(identity) = &observation.launch else {
            return Err(Error::InvalidLaunchBinding);
        };
        if (observation.exited && !allow_exited)
            || validate_name(&observation.summary.name).is_err()
            || !nonce(full_launch_id)
            || !nonce(expected_instance_id)
            || identity.source != source
            || identity.launch_id != full_launch_id
            || observation.instance_id.as_deref() != Some(expected_instance_id)
        {
            return Err(Error::InvalidLaunchBinding);
        }
        Ok(Self {
            name: observation.summary.name.clone(),
            identity: identity.clone(),
            instance_id: expected_instance_id.to_owned(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn source(&self) -> Source {
        self.identity.source
    }
    pub fn launch_id(&self) -> &str {
        &self.identity.launch_id
    }
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub(crate) fn matches_record(&self, record: &HostRecord) -> bool {
        record.name == self.name
            && record.meta.instance_id.as_ref() == Some(&self.instance_id)
            && record.meta.launch == LaunchState::Declared(self.identity.clone())
    }

    fn envelope(&self, token: Option<&str>, request: Value) -> Value {
        json!({"op":"launch_guard_v1", "token":token.unwrap_or_default(),
            "expected_instance_id":self.instance_id, "expected_source":self.source(),
            "expected_launch_id":self.launch_id(), "request":request})
    }

    pub(crate) fn check_ack(&self, value: &Value) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Ack {
            version: u8,
            instance_id: String,
            source: String,
            launch_id: String,
        }
        let ack: Ack = serde_json::from_value(
            value
                .get("launch_guard")
                .cloned()
                .ok_or(Error::LaunchGuardNotAcknowledged)?,
        )
        .map_err(|_| Error::LaunchGuardNotAcknowledged)?;
        if ack.version != 1
            || ack.instance_id != self.instance_id
            || ack.source != self.source().as_str()
            || ack.launch_id != self.launch_id()
        {
            return Err(Error::LaunchGuardNotAcknowledged);
        }
        Ok(())
    }
}

fn nonce(value: &str) -> bool {
    value.len() >= 16 && identifier(value, 128)
}

impl HostClient {
    /// Read-only guarded status, including an exited instance. The temporary
    /// target is never exposed, so this does not grant writable authority from
    /// an exited observation or turn an unguarded probe into exit evidence.
    pub async fn status_launch(
        &self,
        name: &str,
        source: Source,
        launch_id: &str,
        instance_id: &str,
    ) -> Result<HostObservation> {
        timeout(self.limits.operation_timeout, async {
            let observed = self.probe(name).await?;
            let target =
                LaunchTarget::checked_observation(&observed, source, launch_id, instance_id, true)?;
            let record = self.read_record(name).await?.ok_or(Error::NotFound)?;
            if !target.matches_record(&record) {
                return Err(Error::IdentityChanged);
            }
            let mut stream = self.connect(&record).await?;
            wire::send_json(
                &mut stream,
                &target.envelope(record.token.as_deref(), json!({"op":"info"})),
                self.limits.max_line_bytes,
            )
            .await?;
            let reply =
                wire::read_json(&mut stream, &mut Vec::new(), self.limits.max_line_bytes).await?;
            check_ok(&reply)?;
            target.check_ack(&reply)?;
            self.observation_reply(&record, reply).await
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
    /// Never retries or downgrades to raw/name-only or native-bound control.
    /// Timeout/EOF/missing ACK after submission can follow a completed effect.
    pub async fn request_launch(
        &self,
        target: &LaunchTarget,
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
            // Info must not claim another instance even when its outer ACK is
            // correct. Mutable terminal dimensions are checked by parse_reply.
            if matches!(operation, ControlOp::Info) {
                let info: HostRecord = serde_json::from_value(reply["info"].clone())
                    .map_err(|_| Error::InvalidReply)?;
                if !record.same_identity(&info) {
                    return Err(Error::IdentityChanged);
                }
            }
            if let ControlOp::Rename { to } = &operation
                && reply["name"].as_str() != Some(to.as_str())
            {
                return Err(Error::InvalidReply);
            }
            parse_reply(operation, reply)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    /// Only expose replay and the writable handle after the complete launch
    /// guard ACK matches. Buffered frames stay attached to this exact stream.
    pub async fn attach_launch(
        &self,
        target: &LaunchTarget,
        size: TerminalSize,
        replay: bool,
    ) -> Result<Attachment> {
        self.attach_launch_mode(target, size, replay, AttachMode::Bytes)
            .await
    }

    /// `attach_launch` with an explicit stream mode (`Grid` streams JSON lines).
    pub async fn attach_launch_mode(
        &self,
        target: &LaunchTarget,
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
