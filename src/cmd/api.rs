use std::path::Path;

use anyhow::{Result, bail};

use crate::ch::Client;
use crate::vm;

#[derive(clap::Args)]
pub struct Args {
    vm: String,
    /// Endpoint below /api/v1/, e.g. `vm.info`, `vm.counters`, `vm.resize`.
    endpoint: String,
    /// HTTP method. Defaults to PUT when a body is given, GET otherwise.
    #[arg(short = 'X', long)]
    method: Option<String>,
    /// JSON request body.
    #[arg(short, long)]
    data: Option<String>,
}

pub fn run(state_dir: &Path, args: Args) -> Result<()> {
    let vm = vm::find(state_dir, &args.vm)?;
    let method = args
        .method
        .unwrap_or_else(|| if args.data.is_some() { "PUT" } else { "GET" }.into())
        .to_uppercase();
    let resp =
        Client::new(vm.api_socket()).request(&method, &args.endpoint, args.data.as_deref())?;

    let body = match serde_json::from_str::<serde_json::Value>(&resp.body) {
        Ok(json) => serde_json::to_string_pretty(&json)?,
        Err(_) => resp.body,
    };
    if !body.is_empty() {
        println!("{body}");
    }
    if !(200..300).contains(&resp.status) {
        bail!("HTTP {}", resp.status);
    }
    Ok(())
}
