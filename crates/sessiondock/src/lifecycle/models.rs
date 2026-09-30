//! Read-only model catalogs for the new-session picker. Each CLI's own list:
//! Codex and Grok keep a models cache in their home, OpenCode prints its
//! configured providers' models, Claude has no list and takes aliases. Nothing
//! here is a policy: the chosen model is passed to the CLI, which decides.

use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use super::{launcher::CliProfile, model::Source};

/// `opencode models` may first start OpenCode's background service.
const OPENCODE_MODELS_TIMEOUT: Duration = Duration::from_secs(15);
/// Claude's documented `--effort` levels.
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Claude's `--model` aliases for the latest model of each family.
const CLAUDE_ALIASES: [(&str, &str); 4] = [
    ("fable", "Fable"),
    ("opus", "Opus"),
    ("sonnet", "Sonnet"),
    ("haiku", "Haiku"),
];
/// Display order of effort levels when several models are merged.
const EFFORT_ORDER: [&str; 9] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra", "auto",
];

#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub efforts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

#[derive(Serialize, Debug, Default, PartialEq, Eq)]
pub struct Catalog {
    pub models: Vec<Model>,
    /// Levels offered while the CLI's own default model stays selected.
    pub efforts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
}

/// Whether `--effort`/`-c model_reasoning_effort`/`--reasoning-effort` exist.
pub fn supports_effort(source: Source) -> bool {
    matches!(source, Source::Claude | Source::Codex | Source::Grok)
}

pub fn catalog(profile: &CliProfile) -> Catalog {
    let mut catalog = match profile.source {
        Source::Claude => claude(profile),
        Source::Codex => codex_profile(profile),
        Source::Grok => grok(&cli_home(profile, "GROK_HOME", ".grok")),
        Source::Opencode => opencode(profile),
        Source::Shell => Catalog::default(),
    };
    if catalog.efforts.is_empty() {
        catalog.efforts = merged_efforts(&catalog.models);
    }
    catalog
}

/// The CLI's home as its child process would see it: the profile's
/// environment over the service's, then the conventional directory.
pub(super) fn cli_home(profile: &CliProfile, variable: &str, fallback: &str) -> PathBuf {
    let removed = |name: &str| profile.env_remove.iter().any(|removed| removed == name);
    let value = |name: &str| {
        profile
            .env
            .get(name)
            .map(PathBuf::from)
            .or_else(|| {
                (!removed(name))
                    .then(|| std::env::var_os(name))
                    .flatten()
                    .map(PathBuf::from)
            })
            .filter(|path| !path.as_os_str().is_empty())
    };
    value(variable).unwrap_or_else(|| {
        value("HOME")
            .or_else(|| value("USERPROFILE"))
            .unwrap_or_default()
            .join(fallback)
    })
}

fn profile_env(profile: &CliProfile, key: &str) -> Option<String> {
    profile
        .env
        .get(key)
        .cloned()
        .or_else(|| {
            (!profile.env_remove.iter().any(|removed| removed == key))
                .then(|| std::env::var(key).ok())
                .flatten()
        })
        .filter(|value| !value.is_empty())
}

fn claude(profile: &CliProfile) -> Catalog {
    let efforts: Vec<String> = CLAUDE_EFFORTS.iter().map(|&e| e.to_owned()).collect();
    let settings =
        read_json(&cli_home(profile, "CLAUDE_CONFIG_DIR", ".claude").join("settings.json"));
    let default_model = profile_env(profile, "ANTHROPIC_MODEL")
        .or_else(|| settings.as_ref().and_then(|value| text(value, "model")))
        .or_else(|| profile_env(profile, "ANTHROPIC_DEFAULT_MODEL"))
        .filter(|model| {
            !matches!(model.as_str(), "default" | "inherit" | "opusplan")
                && !model.chars().any(char::is_whitespace)
        });
    let configured_effort =
        profile_env(profile, "CLAUDE_CODE_EFFORT_LEVEL").filter(|effort| efforts.contains(effort));
    let effort_for = |id: &str| {
        configured_effort.clone().or_else(|| {
            settings
                .as_ref()
                .and_then(|settings| settings.get("modelSettings"))
                .and_then(|models| models.get(id))
                .and_then(|model| text(model, "effortLevel"))
                .filter(|effort| efforts.contains(effort))
        })
    };
    let mut models: Vec<Model> = CLAUDE_ALIASES
        .iter()
        .map(|&(id, name)| Model {
            id: id.to_owned(),
            name: name.to_owned(),
            efforts: efforts.clone(),
            default_effort: effort_for(id),
        })
        .collect();
    if let Some(id) = default_model
        .as_ref()
        .filter(|id| !models.iter().any(|model| &model.id == *id))
    {
        models.push(Model {
            id: id.clone(),
            name: id.clone(),
            efforts: efforts.clone(),
            default_effort: effort_for(id),
        });
    }
    Catalog {
        models,
        efforts,
        default_model,
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
fn text(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// A cache written by another installed CLI version is not its model catalog.
/// Newer CLIs can expose their bundled catalog without network access or writes
/// to the CLI home. Older CLIs retain the read-only cache fallback.
fn codex_profile(profile: &CliProfile) -> Catalog {
    let home = cli_home(profile, "CODEX_HOME", ".codex");
    let cache = read_json(&home.join("models_cache.json"));
    let bundled = run_bounded(
        profile,
        &home,
        &["debug", "models", "--bundled"],
        Duration::from_secs(3),
    )
    .and_then(|output| serde_json::from_str::<serde_json::Value>(&output).ok())
    .filter(|value| value.get("models").is_some_and(serde_json::Value::is_array));
    let catalog = if let Some(bundled) = bundled {
        let version = run_bounded(profile, &home, &["--version"], Duration::from_secs(3));
        let version = version
            .as_deref()
            .and_then(|value| value.split_whitespace().nth(1));
        if version.is_some()
            && cache
                .as_ref()
                .and_then(|value| value.get("client_version"))
                .and_then(serde_json::Value::as_str)
                == version
        {
            cache
        } else {
            Some(bundled)
        }
    } else {
        cache
    };
    codex_catalog(&home, catalog)
}

/// `$CODEX_HOME/models_cache.json`: listed models only, with their
/// reasoning levels; the default model is `config.toml`'s top-level `model`.
fn codex_catalog(home: &Path, catalog: Option<serde_json::Value>) -> Catalog {
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap_or_default();
    let default_model = toml_top_level_string(&config, "model");
    // An omitted launch effort inherits the user's configuration, even when
    // the picker explicitly selects a different model. The cache only supplies
    // the fallback; treating it as the effective default mislabels the launch.
    let configured_effort = toml_top_level_string(&config, "model_reasoning_effort");
    let models = catalog
        .and_then(|cache| cache.get("models")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|model| text(model, "visibility").is_none_or(|v| v == "list"))
        .filter_map(|model| {
            let id = text(model, "slug")?;
            Some(Model {
                name: text(model, "display_name").unwrap_or_else(|| id.clone()),
                efforts: model
                    .get("supported_reasoning_levels")
                    .and_then(serde_json::Value::as_array)
                    .map(|levels| {
                        levels
                            .iter()
                            .filter_map(|level| text(level, "effort"))
                            .collect()
                    })
                    .unwrap_or_default(),
                default_effort: configured_effort
                    .clone()
                    .or_else(|| text(model, "default_reasoning_level")),
                id,
            })
        })
        .collect();
    Catalog {
        models,
        efforts: Vec::new(),
        default_model,
    }
}

/// A top-level `key = "value"` of a TOML file, before any table header.
fn toml_top_level_string(config: &str, key: &str) -> Option<String> {
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            return None;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        let value = value.trim();
        let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let rest = &value[1..];
        return rest
            .find(quote)
            .map(|end| rest[..end].to_owned())
            .filter(|value| !value.is_empty());
    }
    None
}

/// `$GROK_HOME/models_cache.json`: a map of model id to `info`, with
/// hidden models skipped and each model's reasoning efforts.
pub(super) fn grok(home: &Path) -> Catalog {
    let Some(cache) = read_json(&home.join("models_cache.json")) else {
        return Catalog::default();
    };
    let entries: Vec<serde_json::Value> = match cache.get("models") {
        Some(serde_json::Value::Object(map)) => map.values().cloned().collect(),
        Some(serde_json::Value::Array(list)) => list.clone(),
        _ => Vec::new(),
    };
    let models = entries
        .iter()
        .map(|entry| entry.get("info").unwrap_or(entry))
        .filter(|info| info.get("hidden").and_then(serde_json::Value::as_bool) != Some(true))
        .filter_map(|info| {
            let id = text(info, "id").or_else(|| text(info, "model"))?;
            let supports = info
                .get("supports_reasoning_effort")
                .and_then(serde_json::Value::as_bool)
                != Some(false);
            let levels = info
                .get("reasoning_efforts")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let efforts: Vec<String> = if supports {
                let mut efforts: Vec<String> = levels
                    .iter()
                    .filter_map(|level| text(level, "value").or_else(|| text(level, "id")))
                    .collect();
                efforts.sort_by_key(|effort| effort_rank(effort));
                efforts
            } else {
                Vec::new()
            };
            let default_effort = levels
                .iter()
                .find(|level| {
                    level.get("default").and_then(serde_json::Value::as_bool) == Some(true)
                })
                .and_then(|level| text(level, "value").or_else(|| text(level, "id")))
                .or_else(|| text(info, "reasoning_effort"))
                .filter(|_| supports);
            Some(Model {
                name: text(info, "name").unwrap_or_else(|| id.clone()),
                efforts,
                default_effort,
                id,
            })
        })
        .collect();
    Catalog {
        models,
        efforts: Vec::new(),
        default_model: None,
    }
}

/// `opencode models`: one `provider/model` per line. OpenCode selects a
/// reasoning variant per model, so no effort is offered.
fn opencode(profile: &CliProfile) -> Catalog {
    let cwd = cli_home(profile, "HOME", "");
    let output =
        run_bounded(profile, &cwd, &["models"], OPENCODE_MODELS_TIMEOUT).unwrap_or_default();
    Catalog {
        models: opencode_lines(&output),
        efforts: Vec::new(),
        default_model: None,
    }
}

pub(super) fn opencode_lines(output: &str) -> Vec<Model> {
    let mut seen = std::collections::BTreeSet::new();
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.split_once('/')
                .is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty())
                && !line.contains(char::is_whitespace)
        })
        .filter(|line| seen.insert(line.to_string()))
        .map(|line| Model {
            id: line.to_owned(),
            name: line.to_owned(),
            efforts: Vec::new(),
            default_effort: None,
        })
        .collect()
}

fn effort_rank(effort: &str) -> usize {
    EFFORT_ORDER
        .iter()
        .position(|known| *known == effort)
        .unwrap_or(EFFORT_ORDER.len())
}

fn merged_efforts(models: &[Model]) -> Vec<String> {
    let mut merged: BTreeMap<(usize, String), ()> = BTreeMap::new();
    for effort in models.iter().flat_map(|model| &model.efforts) {
        merged.insert((effort_rank(effort), effort.clone()), ());
    }
    merged.into_keys().map(|(_, effort)| effort).collect()
}

/// The profile's executable with its fixed arguments and environment,
/// stdout collected while it runs, killed at the deadline.
fn run_bounded(
    profile: &CliProfile,
    cwd: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<String> {
    let executable = super::launcher::current_executable(&profile.executable).ok()?;
    let mut command = Command::new(executable);
    command.args(&profile.args).args(args);
    for name in &profile.env_remove {
        command.env_remove(name);
    }
    command.envs(&profile.env);
    for name in super::launcher::DENIED_ENV {
        command.env_remove(name);
    }
    if cwd.is_dir() {
        command.current_dir(cwd);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader
        .join()
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// A model or effort the CLI receives as one argument value: nonempty,
/// printable, no whitespace, and never read as an option.
pub fn valid_choice(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 200
        && !text.starts_with('-')
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}
