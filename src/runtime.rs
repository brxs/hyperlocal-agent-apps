use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use wasmtime::component::{Component, Instance, Linker, Val, types::ComponentItem};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

use crate::{
    values,
    web::{self, WebMount},
};

const FUEL: u64 = 10_000_000;
pub const MAX_COMPONENT_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionInfo {
    /// Exact export names, including the interface name when present.
    pub path: Vec<String>,
    pub parameters: Vec<ParameterInfo>,
    pub results: Vec<Value>,
    pub callable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParameterInfo {
    pub name: String,
    pub ty: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginInfo {
    pub id: String,
    pub path: String,
    pub functions: Vec<FunctionInfo>,
    pub wit: String,
    pub web_url: Option<String>,
    pub sha256: String,
}

struct Plugin {
    web_mount: Option<Arc<WebMount>>,
    live: Mutex<LiveInstance>,
    bytes: Arc<[u8]>,
    info: PluginInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SharedPlugin {
    pub id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub function_count: usize,
    #[serde(default)]
    pub web_available: bool,
    #[serde(default)]
    pub calls_available: bool,
}

struct LiveInstance {
    store: Store<StoreLimits>,
    instance: Instance,
    fault: Option<String>,
}

/// Registry locks cover lookup/publication only. Each plugin serializes access to its live Store.
#[derive(Clone)]
pub struct Runtime {
    engine: Engine,
    plugins: Arc<RwLock<BTreeMap<String, Arc<Plugin>>>>,
}

impl Runtime {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true).consume_fuel(true);
        Ok(Self {
            engine: Engine::new(&config)?,
            plugins: Arc::default(),
        })
    }

    /// Compile and validate before publishing. A failed replacement preserves the old plugin.
    pub fn connect(&self, id: &str, path: impl AsRef<Path>) -> Result<PluginInfo> {
        ensure!(
            !id.is_empty()
                && id.len() <= 128
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
            "plugin id must be 1–128 ASCII letters, digits, dots, underscores or hyphens"
        );
        let path = path
            .as_ref()
            .canonicalize()
            .context("cannot resolve component path")?;
        ensure!(path.is_file(), "component path must be a file");
        ensure!(
            path.metadata()?.len() <= MAX_COMPONENT_BYTES,
            "component exceeds 32 MiB limit"
        );
        let bytes = std::fs::read(&path).context("cannot read component")?;
        // Accept binary components and textual component fixtures. No native/AOT artifacts.
        let bytes = wat::parse_bytes(&bytes).context("invalid WebAssembly binary or text")?;
        ensure!(
            bytes.len() as u64 <= MAX_COMPONENT_BYTES,
            "encoded component exceeds 32 MiB limit"
        );
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let component = Component::new(&self.engine, &bytes)
            .map_err(anyhow::Error::from)
            .context("cannot compile component")?;
        let ty = component.component_type();
        let imports = ty
            .imports(&self.engine)
            .map(|(name, _)| name.to_owned())
            .collect::<Vec<_>>();
        ensure!(
            imports.is_empty(),
            "v0.1 has no host capability bindings; unresolved imports: {}",
            imports.join(", ")
        );
        let decoded = wit_component::decode(&bytes).context("cannot decode component WIT")?;
        let resolve = decoded.resolve();
        let package = decoded.package();
        let nested = resolve
            .packages
            .iter()
            .filter_map(|(id, _)| (id != package).then_some(id))
            .collect::<Vec<_>>();
        let mut printer = wit_component::WitPrinter::default();
        printer
            .print(resolve, package, &nested)
            .context("cannot render WIT")?;
        let mut functions = Vec::new();
        for (name, item) in ty.exports(&self.engine) {
            self.discover(item, vec![name.to_owned()], &mut functions);
        }
        functions.sort_by(|a, b| a.path.cmp(&b.path));
        let serves_web = web::validate(&functions)?;
        let live = self.instantiate(&component)?;
        let web_mount = if serves_web {
            let existing = self
                .plugins
                .read()
                .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?
                .get(id)
                .and_then(|p| p.web_mount.clone());
            if let Some(mount) = existing {
                Some(mount)
            } else {
                let plugins = Arc::downgrade(&self.plugins);
                let engine = self.engine.clone();
                let id = id.to_owned();
                Some(Arc::new(WebMount::start(move |request| {
                    let plugins = plugins.upgrade().context("plugin runtime has stopped")?;
                    let runtime = Runtime {
                        engine: engine.clone(),
                        plugins,
                    };
                    let arguments = json!({"request": request}).as_object().unwrap().clone();
                    web::decode_response(runtime.call(
                        &id,
                        &[web::INTERFACE.into(), web::FUNCTION.into()],
                        &arguments,
                    )?)
                })?))
            }
        } else {
            None
        };
        let info = PluginInfo {
            id: id.to_owned(),
            path: path.display().to_string(),
            functions,
            wit: printer.output.to_string(),
            web_url: web_mount.as_ref().map(|mount| mount.url.clone()),
            sha256: digest,
        };
        self.plugins
            .write()
            .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?
            .insert(
                id.to_owned(),
                Arc::new(Plugin {
                    web_mount,
                    live: Mutex::new(live),
                    bytes: Arc::from(bytes.as_ref()),
                    info: info.clone(),
                }),
            );
        Ok(info)
    }

    /// Only immutable original component bytes are shared, never paths or live guest state.
    pub fn shared_catalog(&self) -> Result<Vec<SharedPlugin>> {
        let plugins = self
            .plugins
            .read()
            .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?;
        Ok(plugins
            .values()
            .map(|p| SharedPlugin {
                id: p.info.id.clone(),
                sha256: p.info.sha256.clone(),
                size_bytes: p.bytes.len() as u64,
                function_count: p.info.functions.len(),
                web_available: p.info.web_url.is_some(),
                calls_available: false,
            })
            .collect())
    }

    pub fn shared_component(&self, sha256: &str) -> Result<Option<Arc<[u8]>>> {
        let plugins = self
            .plugins
            .read()
            .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?;
        Ok(plugins
            .values()
            .find(|p| p.info.sha256 == sha256)
            .map(|p| p.bytes.clone()))
    }

    fn instantiate(&self, component: &Component) -> Result<LiveInstance> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(64 * 1024 * 1024)
            .table_elements(10_000)
            .instances(100)
            .memories(10)
            .tables(10)
            .trap_on_grow_failure(true)
            .build();
        let mut store = Store::new(&self.engine, limits);
        store.limiter(|limits| limits);
        store.set_fuel(FUEL)?;
        let instance = Linker::<StoreLimits>::new(&self.engine)
            .instantiate(&mut store, component)
            .map_err(anyhow::Error::from)
            .context("component instantiation failed")?;
        Ok(LiveInstance {
            store,
            instance,
            fault: None,
        })
    }

    fn discover(&self, item: ComponentItem, path: Vec<String>, functions: &mut Vec<FunctionInfo>) {
        match item {
            ComponentItem::ComponentFunc(f) => functions.push(FunctionInfo {
                path,
                parameters: f
                    .params()
                    .map(|(name, ty)| ParameterInfo {
                        name: name.to_owned(),
                        ty: values::describe(&ty),
                    })
                    .collect(),
                results: f.results().map(|ty| values::describe(&ty)).collect(),
                callable: f.params().all(|(_, t)| values::supported(&t))
                    && f.results().all(|t| values::supported(&t)),
            }),
            ComponentItem::ComponentInstance(instance) => {
                for (name, item) in instance.exports(&self.engine) {
                    let mut path = path.clone();
                    path.push(name.to_owned());
                    self.discover(item, path, functions);
                }
            }
            _ => {}
        }
    }

    fn plugin(&self, id: &str) -> Result<Arc<Plugin>> {
        self.plugins
            .read()
            .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?
            .get(id)
            .cloned()
            .with_context(|| format!("unknown plugin {id:?}"))
    }

    pub fn inspect(&self, id: &str) -> Result<PluginInfo> {
        Ok(self.plugin(id)?.info.clone())
    }

    pub fn list(&self) -> Result<Value> {
        let plugins = self
            .plugins
            .read()
            .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?;
        Ok(
            json!({"plugins": plugins.values().map(|p| json!({"id": p.info.id, "path": p.info.path, "function_count": p.info.functions.len(), "web_url": p.info.web_url})).collect::<Vec<_>>()}),
        )
    }

    pub fn call(
        &self,
        id: &str,
        path: &[String],
        arguments: &serde_json::Map<String, Value>,
    ) -> Result<Value> {
        self.call_snapshot(self.plugin(id)?, path, arguments)
    }

    pub fn call_versioned(
        &self,
        id: &str,
        digest: &str,
        path: &[String],
        arguments: &serde_json::Map<String, Value>,
    ) -> Result<Value> {
        let plugin = self.plugin(id)?;
        ensure!(
            plugin.info.sha256 == digest,
            "peer plugin version changed; inspect and reconnect before calling again"
        );
        self.call_snapshot(plugin, path, arguments)
    }

    fn call_snapshot(
        &self,
        plugin: Arc<Plugin>,
        path: &[String],
        arguments: &serde_json::Map<String, Value>,
    ) -> Result<Value> {
        let id = &plugin.info.id;
        let info = plugin
            .info
            .functions
            .iter()
            .find(|f| f.path == path)
            .context("unknown function path; inspect_plugin returns exact paths")?;
        ensure!(
            info.callable,
            "function uses types unsupported in v0.1 (resources, maps, futures or streams)"
        );
        ensure!(
            arguments.len() == info.parameters.len(),
            "argument names must match function parameters exactly"
        );
        let mut live = plugin.live.lock().map_err(|_| {
            anyhow::anyhow!("plugin instance lock poisoned; reconnect plugin {id:?}")
        })?;
        if let Some(fault) = &live.fault {
            bail!(
                "plugin {id:?} is faulted; reconnect it to create a new instance (state will reset). Previous failure: {fault}"
            );
        }
        let LiveInstance {
            store, instance, ..
        } = &mut *live;
        let mut export = None;
        for name in path {
            export = Some(
                instance
                    .get_export_index(&mut *store, export.as_ref(), name)
                    .context("export not found")?,
            );
        }
        let Some(export) = export else {
            bail!("function path cannot be empty")
        };
        let func = instance
            .get_func(&mut *store, export)
            .context("export is not a function")?;
        let ty = func.ty(&*store);
        let parameters = ty
            .params()
            .map(|(name, ty)| {
                let value = arguments
                    .get(name)
                    .with_context(|| format!("missing argument {name}"))?;
                values::decode(&ty, value).with_context(|| format!("argument {name}"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut results = vec![Val::Bool(false); ty.results().len()];
        // Refill the budget per operation without recreating guest memory or globals.
        store.set_fuel(FUEL)?;
        if let Err(error) = func.call(&mut *store, &parameters, &mut results) {
            live.fault = Some(format!("{error:#}"));
            bail!(
                "component call failed: {error:#}; plugin {id:?} is now faulted. Reconnect it to reset its instance"
            );
        }
        drop(live);
        // Wasmtime 45 performs canonical post-return inside Func::call.
        let results = results
            .iter()
            .map(values::encode)
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"results": results}))
    }
}
