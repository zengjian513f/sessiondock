//! Read the native Factory account; never refresh or rewrite CLI credentials.
use super::{CliProfile, Value, bucket, json, snapshot};
use aes_gcm::{
    AesGcm,
    aead::{Aead, KeyInit, consts::U16},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{path::PathBuf, time::Duration};

fn environment(profile: &CliProfile, key: &str) -> Option<String> {
    if profile.env_remove.iter().any(|name| name == key) {
        return None;
    }
    profile
        .env
        .get(key)
        .cloned()
        .or_else(|| std::env::var(key).ok())
}

pub(super) async fn query(profile: &CliProfile) -> Result<Value, ()> {
    let token = if let Some(token) =
        environment(profile, "FACTORY_API_KEY").filter(|s| !s.is_empty())
    {
        token
    } else {
        let home = environment(profile, "FACTORY_HOME_OVERRIDE")
            .or_else(|| environment(profile, "HOME"))
            .ok_or(())?;
        let root = PathBuf::from(home).join(".factory");
        let key = tokio::fs::read_to_string(root.join("auth.v2.key"))
            .await
            .map_err(|_| ())?;
        let key = STANDARD.decode(key.trim()).map_err(|_| ())?;
        let file = tokio::fs::read_to_string(root.join("auth.v2.file"))
            .await
            .map_err(|_| ())?;
        let parts = file
            .trim()
            .split(':')
            .map(|s| STANDARD.decode(s).map_err(|_| ()))
            .collect::<Result<Vec<_>, _>>()?;
        if parts.len() != 3 || parts[0].len() != 16 || parts[1].len() != 16 {
            return Err(());
        }
        let cipher = AesGcm::<aes_gcm::aes::Aes256, U16>::new_from_slice(&key).map_err(|_| ())?;
        let mut encrypted = parts[2].clone();
        encrypted.extend_from_slice(&parts[1]);
        let plaintext = cipher
            .decrypt(parts[0].as_slice().into(), encrypted.as_slice())
            .map_err(|_| ())?;
        let auth: Value = serde_json::from_slice(&plaintext).map_err(|_| ())?;
        auth["access_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(())?
            .to_owned()
    };
    let base =
        environment(profile, "FACTORY_API_BASE").unwrap_or_else(|| "https://api.factory.ai".into());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ())?;
    let response = client
        .get(format!("{}/api/billing/limits", base.trim_end_matches('/')))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| ())?
        .error_for_status()
        .map_err(|_| ())?;
    let data: Value = response.json().await.map_err(|_| ())?;
    let mut buckets = Vec::new();
    for (group, label) in [("standard", "Standard"), ("core", "Core")] {
        for (window, name) in [
            ("fiveHour", "5h"),
            ("weekly", "weekly"),
            ("monthly", "monthly"),
        ] {
            let row = &data["limits"][group][window];
            if !row.is_object() {
                continue;
            }
            let reset = row["windowEnd"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|t| json!(t.to_rfc3339()))
                .unwrap_or(Value::Null);
            buckets.push(bucket(
                &format!("{group}:{window}"),
                label,
                json!(name),
                row["usedPercent"].as_f64().map(|used| 100. - used),
                reset,
            ));
        }
    }
    if buckets.is_empty() {
        return Err(());
    }
    Ok(snapshot("droid", buckets))
}
