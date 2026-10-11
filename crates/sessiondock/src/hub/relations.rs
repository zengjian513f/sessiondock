//! Resolve fleet record identities before forwarding an edge to its receiving node.
use super::{Client, Registry, aggregate, namespace, proxy::ProxyError};
use crate::metadata::{Predecessor, relations};
use serde_json::{Map, Value, json};

pub async fn inventory(registry: &Registry, client: &Client) -> Result<Value, ProxyError> {
    aggregate::sessions(registry, client, &[("force".into(), "1".into())])
        .await
        .map_err(|e| ProxyError::Invalid(e.message().into()))
}

pub async fn prepare(
    registry: &Registry,
    client: &Client,
    body: &mut Map<String, Value>,
) -> Result<(), ProxyError> {
    body.remove("remote_predecessor");
    let uid = body
        .get("uid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let predecessor = body
        .get("predecessor_uid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let remove = body.get("remove") == Some(&json!(true));
    let (owner_node, _) =
        namespace::split(&uid, true).map_err(|e| ProxyError::Invalid(e.to_string()))?;
    let (source_node, source_uid) =
        namespace::split(&predecessor, true).map_err(|e| ProxyError::Invalid(e.to_string()))?;
    if uid == predecessor {
        return Err(ProxyError::Invalid("不能把当前会话设为自己的前序".into()));
    }
    let edge = if remove {
        Predecessor {
            session_uid: source_uid,
            node_id: (source_node != owner_node).then_some(source_node),
            ..Default::default()
        }
    } else {
        let inventory = inventory(registry, client).await?;
        let rows = inventory["sessions"]
            .as_array()
            .ok_or(ProxyError::Upstream)?;
        let find = |uid: &str| rows.iter().find(|r| r["uid"] == uid && r["stale"] != true);
        let owner =
            find(&uid).ok_or_else(|| ProxyError::Invalid("接续会话不存在或机器离线".into()))?;
        let source = find(&predecessor)
            .ok_or_else(|| ProxyError::Invalid("前序会话不存在或机器离线".into()))?;
        if !relations::top_level(owner) || !relations::top_level(source) {
            return Err(ProxyError::Invalid("前后序关系仅用于顶级会话".into()));
        }
        relations::descriptor(source, owner)
    };
    body.insert("remote_predecessor".into(), json!(edge));
    body.remove("predecessor_uid");
    Ok(())
}
