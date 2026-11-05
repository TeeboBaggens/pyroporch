//! Runs a third-party `.wasm` plugin in a sandbox. The plugin has no way to reach the disk, the
//! network or the rest of the app: its only way out is the one `host_call` function, which goes
//! through the same checks as everything else.

use pyroporch_sdk::{ABI_VERSION, HostCall, HostHandle, Outcome, PaintRequest, Plugin, PluginInfo, RunRequest};
use std::sync::Mutex;
use wasmi::{Caller, Config, Engine, Extern, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};

pub const MAX_WASM_BYTES: usize = 32 * 1024 * 1024;
const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
/// Roughly how many instructions one run may use, so a stuck plugin can't hold a key forever.
/// This is the limit the plugin guide publishes, so it is the one plugins are written against.
pub const FUEL_PER_CALL: u64 = 5_000_000_000;

struct State {
    limits: StoreLimits,
    host: Option<HostHandle>,
}

struct Sandbox {
    store: Store<State>,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    free: TypedFunc<(u32, u32), ()>,
    run: TypedFunc<(u32, u32), u64>,
    /// Plugins built before drawing existed don't have this.
    paint: Option<TypedFunc<(u32, u32), u64>>,
}

pub struct WasmPlugin {
    info: PluginInfo,
    fuel: u64,
    sandbox: Mutex<Sandbox>,
}

fn unpack(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

fn read(memory: &[u8], ptr: u32, len: u32) -> Result<Vec<u8>, String> {
    let (start, len) = (ptr as usize, len as usize);
    if len > MAX_MESSAGE_BYTES {
        return Err("the plugin sent too much data".into());
    }
    memory
        .get(start..start.saturating_add(len))
        .map(<[u8]>::to_vec)
        .ok_or_else(|| "the plugin gave a bad memory address".to_string())
}

fn handle_call(host: Option<&HostHandle>, bytes: &[u8]) -> Result<serde_json::Value, String> {
    let host = host.ok_or("the plugin may only call the app while running an action")?;
    let call: HostCall = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    match call {
        HostCall::Http { request } => {
            serde_json::to_value(host.http(&request)?).map_err(|e| e.to_string())
        }
        HostCall::Log { level, message } => {
            host.log(level, &message);
            Ok(serde_json::Value::Null)
        }
        HostCall::NowMs => Ok(host.now_ms().into()),
    }
}

/// Runs a call from the plugin, and puts the JSON reply into the plugin's own memory.
fn host_call(mut caller: Caller<'_, State>, ptr: u32, len: u32) -> Result<u64, wasmi::Error> {
    let trap = |text: &str| wasmi::Error::new(text.to_string());
    let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
        return Err(trap("the plugin has no memory"));
    };
    let reply: Result<serde_json::Value, String> = read(memory.data(&caller), ptr, len)
        .and_then(|bytes| handle_call(caller.data().host.as_ref(), &bytes));
    let reply = serde_json::to_vec(&reply).map_err(|e| trap(&e.to_string()))?;

    let Some(Extern::Func(alloc)) = caller.get_export("ld_alloc") else {
        return Err(trap("the plugin has no ld_alloc"));
    };
    let alloc = alloc.typed::<u32, u32>(&caller)?;
    let at = alloc.call(&mut caller, reply.len() as u32)?;
    memory
        .write(&mut caller, at as usize, &reply)
        .map_err(|_| trap("the plugin's memory is too small"))?;
    Ok(((at as u64) << 32) | reply.len() as u64)
}

impl WasmPlugin {
    /// Compiles and starts a plugin. Nothing it does here can reach anything: no host is attached.
    pub fn load(bytes: &[u8]) -> Result<Self, String> {
        Self::load_with_fuel(bytes, FUEL_PER_CALL)
    }

    pub fn load_with_fuel(bytes: &[u8], fuel: u64) -> Result<Self, String> {
        if bytes.len() > MAX_WASM_BYTES {
            return Err("the plugin file is too big".into());
        }
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).map_err(|e| format!("not a valid plugin: {e}"))?;
        let limits = StoreLimitsBuilder::new().memory_size(MAX_MEMORY_BYTES).build();
        let mut store = Store::new(&engine, State { limits, host: None });
        store.limiter(|state| &mut state.limits);
        store.set_fuel(fuel).map_err(|e| e.to_string())?;

        let mut linker = Linker::<State>::new(&engine);
        linker
            .func_wrap("pyroporch", "host_call", host_call)
            .map_err(|e| e.to_string())?;
        let instance: Instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| format!("the plugin asks for something the sandbox doesn't offer: {e}"))?;

        let abi = instance
            .get_typed_func::<(), u32>(&store, "ld_abi")
            .map_err(|_| "this file isn't a Pyro Porch plugin".to_string())?
            .call(&mut store, ())
            .map_err(|e| e.to_string())?;
        if abi != ABI_VERSION {
            return Err(format!("the plugin needs plugin system version {abi}; this app has {ABI_VERSION}"));
        }
        let func = "the plugin is missing a required function";
        let memory = instance.get_memory(&store, "memory").ok_or("the plugin has no memory")?;
        let alloc = instance.get_typed_func(&store, "ld_alloc").map_err(|_| func)?;
        let free = instance.get_typed_func(&store, "ld_free").map_err(|_| func)?;
        let run = instance.get_typed_func(&store, "ld_run").map_err(|_| func)?;
        let info_fn = instance.get_typed_func::<(), u64>(&store, "ld_info").map_err(|_| func)?;
        let paint = instance.get_typed_func(&store, "ld_paint").ok();

        let mut sandbox = Sandbox {
            store,
            memory,
            alloc,
            free,
            run,
            paint,
        };
        let packed = info_fn.call(&mut sandbox.store, ()).map_err(|e| e.to_string())?;
        let bytes = sandbox.take_output(packed)?;
        let info: PluginInfo =
            serde_json::from_slice(&bytes).map_err(|e| format!("the plugin described itself badly: {e}"))?;
        Ok(Self {
            info,
            fuel,
            sandbox: Mutex::new(sandbox),
        })
    }
}

impl Sandbox {
    fn take_output(&mut self, packed: u64) -> Result<Vec<u8>, String> {
        let (ptr, len) = unpack(packed);
        let bytes = read(self.memory.data(&self.store), ptr, len)?;
        self.free
            .call(&mut self.store, (ptr, len))
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    }
}

impl Sandbox {
    /// Puts bytes into the plugin's memory, in space it set aside.
    fn hand_over(&mut self, bytes: &[u8]) -> Result<u32, String> {
        let at = self
            .alloc
            .call(&mut self.store, bytes.len() as u32)
            .map_err(|e| format!("the plugin stopped: {e}"))?;
        self.memory
            .write(&mut self.store, at as usize, bytes)
            .map_err(|_| "the plugin's memory is too small".to_string())?;
        Ok(at)
    }
}

impl Plugin for WasmPlugin {
    fn info(&self) -> PluginInfo {
        self.info.clone()
    }

    fn run(&self, request: &RunRequest, host: &HostHandle) -> Result<Outcome, String> {
        let mut sandbox = self.sandbox.lock().map_err(|_| "the plugin stopped working")?;
        let sandbox = &mut *sandbox;
        sandbox.store.set_fuel(self.fuel).map_err(|e| e.to_string())?;
        let input = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        let at = sandbox.hand_over(&input)?;

        sandbox.store.data_mut().host = Some(host.clone());
        let packed = sandbox.run.call(&mut sandbox.store, (at, input.len() as u32));
        sandbox.store.data_mut().host = None;
        let packed = packed.map_err(|e| format!("the plugin stopped: {e}"))?;

        let bytes = sandbox.take_output(packed)?;
        serde_json::from_slice::<Result<Outcome, String>>(&bytes)
            .map_err(|e| format!("the plugin answered badly: {e}"))?
    }

    /// No host is attached here, so while drawing a plugin can't reach anything at all.
    fn paint(&self, request: &PaintRequest) -> Result<Vec<u8>, String> {
        let mut sandbox = self.sandbox.lock().map_err(|_| "the plugin stopped working")?;
        let sandbox = &mut *sandbox;
        let paint = sandbox.paint.ok_or("this plugin doesn't draw")?;
        // A frame gets the same limit as a run, as the plugin guide says.
        sandbox.store.set_fuel(self.fuel).map_err(|e| e.to_string())?;
        let input = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        let at = sandbox.hand_over(&input)?;
        let packed = paint
            .call(&mut sandbox.store, (at, input.len() as u32))
            .map_err(|e| format!("the plugin stopped: {e}"))?;
        sandbox.take_output(packed)
    }
}
