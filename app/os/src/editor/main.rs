// SPDX-License-Identifier: MPL-2.0

//! Desktop-only browser editor. No board, device storage, or real network adapter.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("panelet-studio is only supported on macOS and Linux");

mod assets;
mod runtime;
mod transport;

use std::io::{self, Read, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--render-worker"] {
        // Each request owns a child process; the parent enforces its deadline.
        let mut body = Vec::new();
        io::stdin()
            .take((transport::MAX_BODY + 1) as u64)
            .read_to_end(&mut body)?;
        let reply = if body.len() > transport::MAX_BODY {
            transport::error("request_too_large", "Preview input exceeds 64 KiB")
        } else {
            match serde_json::from_slice(&body) {
                Ok(request) => runtime::render(request),
                Err(_) => transport::error("invalid_json", "Invalid preview JSON"),
            }
        };
        io::stdout().write_all(&serde_json::to_vec(&reply)?)?;
        return Ok(());
    }
    const USAGE: &str = "Usage: panelet-studio [--port 18745] [--web-assets DIR]";
    let mut port = 18745;
    let mut assets_root = None;
    let mut port_seen = false;
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--port" if !port_seen => {
                port = args.next().ok_or(USAGE)?.parse::<u16>()?;
                port_seen = true;
            }
            "--web-assets" if assets_root.is_none() => {
                let root = args
                    .next()
                    .filter(|value| !value.is_empty() && !value.starts_with("--"))
                    .ok_or(USAGE)?;
                assets_root = Some(std::path::PathBuf::from(root));
            }
            "--help" | "-h" => {
                println!("Panelet Studio\n{USAGE}\nDefault: native Rune preview. --web-assets: browser WASM preview with persistent local assets; simulated 8 MiB capacity, development token preview-token.\nOpen the printed loopback URL in a browser. Ctrl-C exits.");
                return Ok(());
            }
            _ => return Err(USAGE.into()),
        }
    }
    transport::serve(port, assets_root)?;
    Ok(())
}
