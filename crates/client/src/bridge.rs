//! Thin wrapper over the Tauri command/event bridge (spec 0009).
//!
//! `trunk serve` in a plain browser has no Tauri runtime; every call here
//! reports that instead of panicking, so the UI still runs for design work.

use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"], js_name = listen, catch)]
    async fn tauri_listen(event: &str, handler: &JsValue) -> Result<JsValue, JsValue>;
}

/// Error from a command, either the core's `{ code, message }` or a bridge
/// failure of our own.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct CommandError {
    /// `invalid_settings`, `not_configured`, `not_connected`, `io` (spec 0009),
    /// or `bridge` when the failure is ours. Shown to the user via `message`;
    /// kept for callers that branch on it.
    #[allow(dead_code)]
    pub code: String,
    pub message: String,
}

impl CommandError {
    fn bridge(message: impl Into<String>) -> Self {
        Self {
            code: "bridge".into(),
            message: message.into(),
        }
    }
}

/// Whether a Tauri runtime is present — false under `trunk serve`.
pub fn available() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    js_sys::Reflect::get(&window, &JsValue::from_str("__TAURI__"))
        .map(|tauri| !tauri.is_undefined())
        .unwrap_or(false)
}

/// Calls a Tauri command with `args` and decodes its result.
pub async fn invoke<A: Serialize, R: DeserializeOwned>(
    cmd: &str,
    args: &A,
) -> Result<R, CommandError> {
    if !available() {
        return Err(CommandError::bridge("нет Tauri — запущено в браузере"));
    }
    let args = serde_wasm_bindgen::to_value(args)
        .map_err(|err| CommandError::bridge(format!("аргументы не сериализуются: {err}")))?;
    match tauri_invoke(cmd, args).await {
        Ok(value) => serde_wasm_bindgen::from_value(value)
            .map_err(|err| CommandError::bridge(format!("ответ не разобран: {err}"))),
        Err(err) => Err(serde_wasm_bindgen::from_value::<CommandError>(err.clone())
            .unwrap_or_else(|_| CommandError::bridge(format!("{err:?}")))),
    }
}

/// Subscribes to a core event. The closure is leaked on purpose: the
/// subscription lives as long as the window does.
pub fn listen<T: DeserializeOwned + 'static>(
    event: &'static str,
    mut on_event: impl FnMut(T) + 'static,
) {
    if !available() {
        return;
    }
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |raw: JsValue| {
        // Tauri wraps every payload as `{ event, id, payload }`.
        let payload = js_sys::Reflect::get(&raw, &JsValue::from_str("payload")).unwrap_or(raw);
        match serde_wasm_bindgen::from_value::<T>(payload) {
            Ok(value) => on_event(value),
            Err(err) => web_sys::console::error_1(&JsValue::from_str(&format!(
                "событие {event} не разобрано: {err}"
            ))),
        }
    });
    let js_handler = handler.as_ref().clone();
    handler.forget();
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = tauri_listen(event, &js_handler).await {
            web_sys::console::error_1(&err);
        }
    });
}

/// UUID v4 from the browser's crypto, which both webviews provide.
pub fn uuid() -> String {
    let fallback = || format!("{:x}", js_sys::Date::now() as u64);
    let Some(window) = web_sys::window() else {
        return fallback();
    };
    let crypto = match js_sys::Reflect::get(&window, &JsValue::from_str("crypto")) {
        Ok(crypto) if !crypto.is_undefined() => crypto,
        _ => return fallback(),
    };
    let method = match js_sys::Reflect::get(&crypto, &JsValue::from_str("randomUUID")) {
        Ok(method) => method,
        Err(_) => return fallback(),
    };
    method
        .dyn_ref::<js_sys::Function>()
        .and_then(|method| method.call0(&crypto).ok())
        .and_then(|value| value.as_string())
        .unwrap_or_else(fallback)
}
