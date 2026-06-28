//! `pulsate-plugin` — the WebAssembly plugin host.
//!
//! A capability-sandboxed Wasmtime host (`docs/12-plugins.md`): it compiles a
//! plugin module, enforces a **fuel** budget (so a plugin cannot burn unbounded
//! CPU — `PLS-PLG-0003`), grants **capabilities** explicitly (an ungranted host
//! import fails instantiation — `PLS-PLG-0004`), and checks the plugin's **ABI
//! version** (`PLS-PLG-0005`) before running it. A plugin exports
//! `pulsate_abi_version() -> i32` and `eval(i32) -> i32`; the host calls `eval`
//! under the fuel limit.
#![forbid(unsafe_code)]

use pulsate_core::Code;
use wasmtime::{Caller, Config, Engine, Linker, Module, Store};

/// The host ABI version a plugin must target.
pub const ABI_VERSION: i32 = 1;

/// Capabilities a plugin may be granted. Anything not granted is denied: the
/// corresponding host import is simply not defined, so a plugin that needs it
/// fails to instantiate.
#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    /// May call `p8::log(i32)`.
    pub log: bool,
}

/// An error running a plugin, carrying a stable `PLS-PLG-*` code.
#[derive(Debug)]
pub struct PluginError {
    /// The stable error code.
    pub code: Code,
    /// A human message (for logs).
    pub message: String,
}

impl PluginError {
    fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for PluginError {}

/// The result of running a plugin's `eval`.
#[derive(Debug, Clone)]
pub struct RunResult {
    /// The value `eval` returned.
    pub output: i32,
    /// Values the plugin sent to `p8::log`.
    pub logs: Vec<i32>,
    /// Fuel consumed by the call.
    pub fuel_used: u64,
}

/// Per-instance host state the sandboxed plugin can touch through host imports.
struct HostState {
    logs: Vec<i32>,
}

/// The plugin host: owns the Wasmtime engine (configured for fuel metering).
pub struct PluginHost {
    engine: Engine,
}

impl PluginHost {
    /// Build a host with fuel metering enabled.
    ///
    /// # Errors
    /// Returns a [`PluginError`] if the engine cannot be created.
    pub fn new() -> Result<Self, PluginError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config)
            .map_err(|e| PluginError::new(Code::PLG_LOAD, format!("engine init failed: {e}")))?;
        Ok(Self { engine })
    }

    /// Compile a plugin from `.wasm` bytes or `.wat` text.
    ///
    /// # Errors
    /// Returns `PLS-PLG-0001` if the module does not compile, or `PLS-PLG-0005`
    /// if it does not export the required ABI/entry functions.
    pub fn load(&self, name: &str, bytes: &[u8]) -> Result<Plugin, PluginError> {
        let module = Module::new(&self.engine, bytes).map_err(|e| {
            PluginError::new(
                Code::PLG_LOAD,
                format!("plugin `{name}` failed to compile: {e}"),
            )
        })?;
        let exports: Vec<String> = module.exports().map(|e| e.name().to_string()).collect();
        for required in ["pulsate_abi_version", "eval"] {
            if !exports.iter().any(|e| e == required) {
                return Err(PluginError::new(
                    Code::PLG_ABI,
                    format!("plugin `{name}` is missing required export `{required}`"),
                ));
            }
        }
        Ok(Plugin {
            module,
            name: name.to_string(),
        })
    }

    /// Run `plugin.eval(input)` under a `fuel` budget with the granted `caps`.
    ///
    /// # Errors
    /// - `PLS-PLG-0005` if the plugin's ABI version does not match the host.
    /// - `PLS-PLG-0004` if it imports a host function it was not granted.
    /// - `PLS-PLG-0003` if it exhausts its fuel budget.
    /// - `PLS-PLG-0002` if it traps for any other reason.
    pub fn run(
        &self,
        plugin: &Plugin,
        caps: Capabilities,
        fuel: u64,
        input: i32,
    ) -> Result<RunResult, PluginError> {
        let mut store = Store::new(&self.engine, HostState { logs: Vec::new() });
        store
            .set_fuel(fuel)
            .map_err(|e| PluginError::new(Code::PLG_LOAD, format!("set_fuel failed: {e}")))?;

        let mut linker = Linker::new(&self.engine);
        if caps.log {
            linker
                .func_wrap("p8", "log", |mut caller: Caller<'_, HostState>, x: i32| {
                    caller.data_mut().logs.push(x);
                })
                .map_err(|e| PluginError::new(Code::PLG_LOAD, format!("linker error: {e}")))?;
        }

        let instance = linker
            .instantiate(&mut store, &plugin.module)
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("unknown import") || msg.contains("incompatible import") {
                    PluginError::new(
                        Code::PLG_CAPABILITY,
                        format!(
                            "plugin `{}` requires an ungranted capability: {msg}",
                            plugin.name
                        ),
                    )
                } else {
                    PluginError::new(Code::PLG_LOAD, format!("instantiate failed: {msg}"))
                }
            })?;

        // ABI check.
        let abi = instance
            .get_typed_func::<(), i32>(&mut store, "pulsate_abi_version")
            .map_err(|e| PluginError::new(Code::PLG_ABI, format!("no ABI export: {e}")))?;
        let version = abi
            .call(&mut store, ())
            .map_err(|e| PluginError::new(Code::PLG_ABI, format!("ABI call trapped: {e}")))?;
        if version != ABI_VERSION {
            return Err(PluginError::new(
                Code::PLG_ABI,
                format!("plugin ABI {version} != host ABI {ABI_VERSION}"),
            ));
        }

        // Run `eval` under the fuel budget.
        let eval = instance
            .get_typed_func::<i32, i32>(&mut store, "eval")
            .map_err(|e| PluginError::new(Code::PLG_LOAD, format!("no eval export: {e}")))?;
        match eval.call(&mut store, input) {
            Ok(output) => {
                let remaining = store.get_fuel().unwrap_or(0);
                Ok(RunResult {
                    output,
                    logs: store.into_data().logs,
                    fuel_used: fuel.saturating_sub(remaining),
                })
            }
            Err(trap) => {
                // Out of fuel leaves the store with zero remaining.
                if store.get_fuel().is_ok_and(|f| f == 0) {
                    Err(PluginError::new(
                        Code::PLG_FUEL,
                        format!("plugin `{}` exhausted its fuel budget", plugin.name),
                    ))
                } else {
                    Err(PluginError::new(
                        Code::PLG_TRAPPED,
                        format!("plugin `{}` trapped: {trap}", plugin.name),
                    ))
                }
            }
        }
    }
}

/// A compiled plugin module.
#[derive(Debug)]
pub struct Plugin {
    module: Module,
    name: String,
}

impl Plugin {
    /// The plugin name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A well-formed plugin: ABI v1, `eval` doubles its input, logs the result.
    const GOOD: &str = r#"
        (module
          (import "p8" "log" (func $log (param i32)))
          (func (export "pulsate_abi_version") (result i32) (i32.const 1))
          (func (export "eval") (param i32) (result i32)
            (call $log (i32.mul (local.get 0) (i32.const 2)))
            (i32.mul (local.get 0) (i32.const 2))))
    "#;

    // No `p8::log` import — runs without the `log` capability.
    const NO_IMPORT: &str = r#"
        (module
          (func (export "pulsate_abi_version") (result i32) (i32.const 1))
          (func (export "eval") (param i32) (result i32)
            (i32.add (local.get 0) (i32.const 1))))
    "#;

    // Wrong ABI version.
    const BAD_ABI: &str = r#"
        (module
          (func (export "pulsate_abi_version") (result i32) (i32.const 99))
          (func (export "eval") (param i32) (result i32) (local.get 0)))
    "#;

    // Spins forever — must be killed by the fuel limit.
    const INFINITE: &str = r#"
        (module
          (func (export "pulsate_abi_version") (result i32) (i32.const 1))
          (func (export "eval") (param i32) (result i32)
            (loop (br 0)) (i32.const 0)))
    "#;

    #[test]
    fn runs_a_plugin_with_log_capability() {
        let host = PluginHost::new().unwrap();
        let plugin = host.load("doubler", GOOD.as_bytes()).unwrap();
        let caps = Capabilities { log: true };
        let r = host.run(&plugin, caps, 100_000, 21).unwrap();
        assert_eq!(r.output, 42);
        assert_eq!(r.logs, vec![42]);
        assert!(r.fuel_used > 0);
    }

    #[test]
    fn ungranted_capability_is_denied() {
        let host = PluginHost::new().unwrap();
        let plugin = host.load("doubler", GOOD.as_bytes()).unwrap();
        // GOOD imports `p8::log` but we grant nothing.
        let err = host
            .run(&plugin, Capabilities::default(), 100_000, 1)
            .unwrap_err();
        assert_eq!(err.code, Code::PLG_CAPABILITY);
    }

    #[test]
    fn plugin_needing_no_capability_runs_sandboxed() {
        let host = PluginHost::new().unwrap();
        let plugin = host.load("inc", NO_IMPORT.as_bytes()).unwrap();
        let r = host
            .run(&plugin, Capabilities::default(), 100_000, 41)
            .unwrap();
        assert_eq!(r.output, 42);
    }

    #[test]
    fn abi_mismatch_is_rejected() {
        let host = PluginHost::new().unwrap();
        let plugin = host.load("old", BAD_ABI.as_bytes()).unwrap();
        let err = host
            .run(&plugin, Capabilities::default(), 100_000, 1)
            .unwrap_err();
        assert_eq!(err.code, Code::PLG_ABI);
    }

    #[test]
    fn fuel_limit_kills_an_infinite_loop() {
        let host = PluginHost::new().unwrap();
        let plugin = host.load("spin", INFINITE.as_bytes()).unwrap();
        let err = host
            .run(&plugin, Capabilities::default(), 10_000, 1)
            .unwrap_err();
        assert_eq!(err.code, Code::PLG_FUEL);
    }

    #[test]
    fn missing_export_is_an_abi_error() {
        let host = PluginHost::new().unwrap();
        let err = host.load("empty", b"(module)").unwrap_err();
        assert_eq!(err.code, Code::PLG_ABI);
    }
}
