// SPDX-License-Identifier: MPL-2.0

// Per-VM os bindings; native calls capture the invocation before constructing cold futures.
use super::*;
use crate::display::DisplayBroker;
#[derive(Clone)]
pub struct OsBindings {
    active: crate::sync::Arc<crate::sync::Mutex<Option<Std>>>,
    pub display: DisplayBroker,
    pub epoch: u64,
}
impl OsBindings {
    pub fn new(display: DisplayBroker, epoch: u64) -> Self {
        Self {
            active: crate::sync::Arc::new(crate::sync::Mutex::new(None)),
            display,
            epoch,
        }
    }
    fn current(&self) -> HostResult<Std> {
        self.active
            .lock()
            .map_err(|_| HostError::cancelled())?
            .clone()
            .ok_or_else(HostError::cancelled)
    }
    pub(crate) fn activate(&self, host: Std, drawing_allowed: bool) -> HostResult<()> {
        {
            let mut drawing = host
                .runtime
                .invocation
                .drawing
                .lock()
                .map_err(|_| HostError::cancelled())?;
            drawing.broker = Some(self.display.clone());
            drawing.epoch = self.epoch;
            drawing.allowed = drawing_allowed;
        }
        *self.active.lock().map_err(|_| HostError::cancelled())? = Some(host);
        Ok(())
    }
    pub(crate) fn revoke(&self) {
        if let Ok(mut active) = self.active.lock() {
            if let Some(host) = active.take() {
                host.revoke();
            }
        }
    }
}
#[allow(deprecated)]
pub(super) fn modules(bindings: OsBindings) -> Result<Vec<Module>, ContextError> {
    let mut modules = vec![
        Module::with_item(["os", "config"])?,
        Module::with_item(["os", "runtime"])?,
        Module::with_item(["os", "time"])?,
        Module::with_item(["os", "http"])?,
        Module::with_item(["os", "storage"])?,
        Module::with_item(["os", "log"])?,
        Module::with_item(["os", "resource"])?,
        Module::with_item(["os", "fs", "private"])?,
        Module::with_item(["os", "fs", "shared"])?,
        Module::with_item(["os", "device"])?,
        Module::with_item(["os", "power"])?,
        Module::with_item(["os", "wifi"])?,
        Module::with_item(["os", "screen"])?,
        Module::with_item(["os", "json"])?,
    ];

    {
        let binding = bindings.clone();
        modules[0]
            .function(["get"], move || {
                let host = binding.current()?;
                config_get(&host.config)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[1]
            .function(["fail"], move |message: &str| {
                let host = binding.current()?;
                runtime_fail(&host.runtime, message)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[2]
            .function(["uptime"], move || {
                let host = binding.current()?;
                time_uptime(&host.time)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[2]
            .function(["unix"], move || {
                let host = binding.current()?;
                time_unix(&host.time)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[2]
            .function(["local"], move || {
                let host = binding.current()?;
                time_local(&host.time)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[2]
            .function(["lunar"], move |year: i64, month: i64, day: i64| {
                let host = binding.current()?;
                time_lunar(&host.time, year, month, day)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[2].async_function(["rtc"], move || {
            let captured = binding.current();
            async move {
                let host = captured?;
                time_rtc(host.time).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[3]
            .function(["request"], move |method: &str, url: &str| {
                let host = binding.current()?;
                http_request(&host.http, method, url)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[3].async_function(["get"], move |url: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                http_get(host.http, url).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[3].async_function(["get_text"], move |url: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                http_get_text(host.http, url).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[3].async_function(["fetch"], move |request: HttpRequest| {
            let captured = binding.current();
            async move {
                let host = captured?;
                http_fetch(host.http, request).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[4].async_function(["has"], move |key: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                storage_has(host.storage, key).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[4].async_function(["get"], move |key: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                storage_get(host.storage, key).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[4].async_function(["set"], move |key: String, value: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                storage_set(host.storage, key, value).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[4].async_function(["delete"], move |key: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                storage_delete(host.storage, key).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[5].async_function(["write"], move |level: String, message: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                log_write(host.log, level, message).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[5].async_function(["debug"], move |message: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                log_debug(host.log, message).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[5].async_function(["info"], move |message: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                log_info(host.log, message).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[5].async_function(["warn"], move |message: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                log_warn(host.log, message).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[5].async_function(["error"], move |message: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                log_error(host.log, message).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[1]
            .function(["abi_version"], move || {
                let host = binding.current()?;
                Ok::<_, HostError>(runtime_abi_version(&host.runtime))
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[1]
            .function(["capability"], move |name: String| {
                let host = binding.current()?;
                Ok::<_, HostError>(runtime_capability(&host.runtime, name))
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[1]
            .function(["remaining_ms"], move || {
                let host = binding.current()?;
                Ok::<_, HostError>(runtime_remaining_ms(&host.runtime))
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[1]
            .function(["cancelled"], move || {
                let host = binding.current()?;
                Ok::<_, HostError>(runtime_cancelled(&host.runtime))
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[1]
            .function(["limits"], move || {
                let host = binding.current()?;
                Ok::<_, HostError>(runtime_limits(&host.runtime))
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[1]
            .function(["ok"], move |value: Value| {
                let host = binding.current()?;
                runtime_ok(&host.runtime, value)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[6].async_function(["exists"], move |path: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                resource_exists(host.resource, path).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[6].async_function(["read_text"], move |path: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                resource_read_text(host.resource, path).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[6].async_function(["list"], move || {
            let captured = binding.current();
            async move {
                let host = captured?;
                resource_list(host.resource).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[7].async_function(["exists"], move |path: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_private_exists(host.fs.private, path).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[7].async_function(["read_text"], move |path: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_private_read_text(host.fs.private, path).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[7].async_function(["write_text"], move |path: String, text: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_private_write_text(host.fs.private, path, text).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[7].async_function(["delete"], move |path: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_private_delete(host.fs.private, path).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[7].async_function(["list"], move |prefix: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_private_list(host.fs.private, prefix).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[8].async_function(["exists"], move |path: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_shared_exists(host.fs.shared, path).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[8].async_function(["read_text"], move |path: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_shared_read_text(host.fs.shared, path).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[8].async_function(["write_text"], move |path: String, text: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_shared_write_text(host.fs.shared, path, text).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[8].async_function(["delete"], move |path: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_shared_delete(host.fs.shared, path).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[8].async_function(["list"], move |prefix: String| {
            let captured = binding.current();
            async move {
                let host = captured?;
                fs_shared_list(host.fs.shared, prefix).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[1].async_function(["sleep"], move |duration_ms: i64| {
            let captured = binding.current();
            async move {
                let host = captured?;
                runtime_sleep(host.runtime, duration_ms).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[1].async_function(["wait_all"], move |value: Value| {
            let captured = binding.current();
            async move {
                let host = match captured {
                    Ok(host) => host,
                    Err(error) => {
                        return VmResult::Ok(Err(error));
                    }
                };
                runtime_wait_all(host.runtime, value).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[1].async_function(["timeout"], move |value: Value, timeout_ms: i64| {
            let captured = binding.current();
            async move {
                let host = match captured {
                    Ok(host) => host,
                    Err(_error) => {
                        return VmResult::err(rune::runtime::VmError::panic(
                            "no active invocation",
                        ));
                    }
                };
                runtime_timeout(host.runtime, value, timeout_ms).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[9]
            .function(["info"], move || {
                let host = binding.current()?;
                device_info(&host.device)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[9]
            .function(["model"], move || {
                let host = binding.current()?;
                device_model(&host.device)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[9]
            .function(["firmware"], move || {
                let host = binding.current()?;
                device_firmware(&host.device)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[9]
            .function(["screen_width"], move || {
                let host = binding.current()?;
                device_screen_width(&host.device)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[9]
            .function(["screen_height"], move || {
                let host = binding.current()?;
                device_screen_height(&host.device)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[9]
            .function(["color_count"], move || {
                let host = binding.current()?;
                device_color_count(&host.device)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[10].async_function(["battery"], move || {
            let captured = binding.current();
            async move {
                let host = captured?;
                power_battery(host.power).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[11].async_function(["status"], move || {
            let captured = binding.current();
            async move {
                let host = captured?;
                wifi_status(host.wifi).await
            }
        })?;
    }
    {
        let binding = bindings.clone();
        modules[12]
            .function(["fit_text"], move |text: &str, width: i64, scale: i64| {
                let host = binding.current()?;
                screen_fit_text(&host.screen, text, width, scale)
            })
            .build()?;
    }
    {
        let binding = bindings.clone();
        modules[12]
            .function(
                ["fit_text_sized"],
                move |text: &str, width: i64, height: i64| {
                    let host = binding.current()?;
                    screen_fit_text_sized(&host.screen, text, width, height)
                },
            )
            .build()?;
    }
    modules[13]
        .function(["parse"], |text: &str| JsonValue::parse(text))
        .build()?;
    screen_module(&mut modules[12], bindings)?;
    Ok(modules)
}
fn screen_module(module: &mut Module, bindings: OsBindings) -> Result<(), ContextError> {
    module
        .function(["get_size"], || vec![400i64, 300])
        .build()?;
    module.function(["get_width"], || 400i64).build()?;
    module.function(["get_height"], || 300i64).build()?;
    module
        .function(["get_palette"], || vec![0i64, 1, 2, 3])
        .build()?;
    for (name, color) in [("BLACK", 0i64), ("WHITE", 1), ("YELLOW", 2), ("RED", 3)] {
        module.constant([name], color).build()?;
    }
    module
        .function(["get_context"], move || {
            let host = bindings.current()?;
            let invocation = host.runtime.invocation.clone();
            invocation.check_cancelled()?;
            if !invocation.allows(CAP_SCREEN_PRESENT) {
                return Err(HostError::capability_denied(CAP_SCREEN_PRESENT));
            }
            {
                let mut drawing = invocation
                    .drawing
                    .lock()
                    .map_err(|_| HostError::cancelled())?;
                if !drawing.allowed {
                    return Err(HostError::cancelled());
                }
                if drawing.frame.is_none() {
                    drawing.frame = Some(
                        bindings
                            .display
                            .acquire_working(bindings.epoch)
                            .map_err(|_| HostError::quota("canvas pool exhausted"))?,
                    );
                }
            }
            Ok(Canvas { invocation })
        })
        .build()?;
    Ok(())
}
