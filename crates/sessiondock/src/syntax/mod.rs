//! Generate optional conversation highlighting from trusted, embedded grammars.
//! Source text and path hints are JS function arguments, never module names or scripts.
mod chunks;
use std::{
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use rquickjs::{Context, Function, Module, Object, Runtime};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Code,
    Command,
    Tool,
}

#[derive(Deserialize)]
pub(crate) struct Block {
    kind: Kind,
    source: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    chunked: bool,
}

#[derive(Serialize)]
pub(crate) struct Highlight {
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<chunks::Chunk>>,
    language: String,
    detected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    languages: Option<Vec<String>>,
}

fn engine() -> rquickjs::Result<Context> {
    let runtime = Runtime::new()?;
    let context = Context::full(&runtime)?;
    context.with(|ctx| -> rquickjs::Result<()> {
        macro_rules! register {
            ($path:literal) => {
                Module::declare(ctx.clone(), $path, include_str!($path))?;
            };
        }
        register!("vendor/highlight/core.min.js");
        register!("vendor/highlight/languages/bash.min.js");
        register!("vendor/highlight/languages/css.min.js");
        register!("vendor/highlight/languages/diff.min.js");
        register!("vendor/highlight/languages/javascript.min.js");
        register!("vendor/highlight/languages/json.min.js");
        register!("vendor/highlight/languages/markdown.min.js");
        register!("vendor/highlight/languages/python.min.js");
        register!("vendor/highlight/languages/sql.min.js");
        register!("vendor/highlight/languages/typescript.min.js");
        register!("vendor/highlight/languages/xml.min.js");
        register!("vendor/highlight/languages/yaml.min.js");
        let module = Module::declare(ctx.clone(), "render.js", include_str!("render.js"))?;
        let (module, ready) = module.eval()?;
        ready.finish::<()>()?;
        ctx.globals()
            .set("renderBlock", module.get::<_, Function>("renderBlock")?)
    })?;
    Ok(context)
}

fn render(
    context: &Context,
    block: Block,
    cancel: CancellationToken,
) -> rquickjs::Result<Option<Highlight>> {
    let kind = match block.kind {
        Kind::Code => "code",
        Kind::Command => "command",
        Kind::Tool => "tool",
    };
    // Cancel abandoned requests and give larger blocks time to finish without
    // letting a stuck grammar monopolize a renderer indefinitely.
    let budget = Duration::from_secs(5 + block.source.len() as u64 / 32_768);
    let start = Instant::now();
    context
        .runtime()
        .set_interrupt_handler(Some(Box::new(move || {
            cancel.is_cancelled() || start.elapsed() >= budget
        })));
    let result = context.with(|ctx| -> rquickjs::Result<Option<Highlight>> {
        let function: Function = ctx.globals().get("renderBlock")?;
        let object: Option<Object> =
            function.call((kind, block.source, block.language, block.path))?;
        object
            .map(|value| {
                let html: String = value.get("html")?;
                let chunks = if block.chunked {
                    chunks::split(&html)
                } else {
                    None
                };
                Ok(Highlight {
                    html: if chunks.is_none() { Some(html) } else { None },
                    chunks,
                    language: value.get("language")?,
                    detected: value.get("detected")?,
                    languages: value.get("languages")?,
                })
            })
            .transpose()
    });
    context.runtime().set_interrupt_handler(None);
    result
}

// Only active batches occupy blocking threads. Return initialized grammar
// contexts to the pool so successive stream updates reuse the compiled modules.
static SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));
static ENGINES: Mutex<Vec<Context>> = Mutex::new(Vec::new());

pub(crate) async fn highlight(blocks: Vec<Block>) -> Result<Vec<Option<Highlight>>, String> {
    let cancel = CancellationToken::new();
    let _on_disconnect = cancel.clone().drop_guard();
    let permit = SLOTS
        .clone()
        .acquire_owned()
        .await
        .map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let cached = ENGINES.lock().unwrap_or_else(|err| err.into_inner()).pop();
        let context = match cached {
            Some(context) => context,
            None => engine().map_err(|err| format!("initialize syntax grammars: {err}"))?,
        };
        let results = blocks
            .into_iter()
            .take_while(|_| !cancel.is_cancelled())
            .map(|block| render(&context, block, cancel.clone()).ok().flatten())
            .collect();
        ENGINES
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .push(context);
        Ok(results)
    })
    .await
    .map_err(|e| e.to_string())?
}
