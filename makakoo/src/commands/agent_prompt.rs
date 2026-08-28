//! Send one prompt to a running DeepSeek Harness slot runtime.

use std::path::PathBuf;
use std::time::Duration;

use makakoo_core::agents::runtime_client::{read_endpoint, run_prompt, RunOutcome};
use makakoo_core::agents::{AgentRuntimeEngine, AgentSlot};

use crate::context::CliContext;

/// One-shot prompts wait longer than a chat turn: the caller is
/// sitting at a terminal and asked for this specific answer.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(600);

pub fn run(ctx: &CliContext, slot_id: &str, text: &str, session_id: &str) -> anyhow::Result<i32> {
    let project_dir = dsh_project_dir(ctx, slot_id)?;
    let endpoint = read_endpoint(&project_dir, slot_id)?;
    let port = endpoint.port;
    let outcome = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let http = reqwest::Client::builder()
                .timeout(PROMPT_TIMEOUT)
                .build()
                .map_err(|e| anyhow::anyhow!("build http client: {e}"))?;
            run_prompt(&http, &endpoint, text, session_id, PROMPT_TIMEOUT)
                .await
                .map_err(|error| {
                    anyhow::anyhow!(
                        "slot '{}' is not responding at 127.0.0.1:{}: {}",
                        slot_id,
                        port,
                        error
                    )
                })
        })
    })?;
    match outcome {
        RunOutcome::Answer(answer) => {
            println!("{}", answer);
            Ok(0)
        }
        RunOutcome::Refused { status, message } => {
            anyhow::bail!("agent runtime returned {}: {}", status, message)
        }
    }
}

pub fn health(ctx: &CliContext, slot_id: &str) -> anyhow::Result<i32> {
    let project_dir = dsh_project_dir(ctx, slot_id)?;
    let endpoint = read_endpoint(&project_dir, slot_id)?;
    let url = format!("{}/health", endpoint.base_url());
    let response = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()?
                .get(url)
                .send()
                .await
        })
    });
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            println!(
                "{}: not running (127.0.0.1:{}: {})",
                slot_id, endpoint.port, error
            );
            return Ok(1);
        }
    };
    if !response.status().is_success() {
        return Ok(1);
    }
    println!("{}: healthy (deepseek-harness)", slot_id);
    Ok(0)
}

fn dsh_project_dir(ctx: &CliContext, slot_id: &str) -> anyhow::Result<PathBuf> {
    let slot_path = makakoo_core::agents::checked_slot_path(ctx.home(), slot_id)?;
    let slot = AgentSlot::load_from_file(&slot_path)
        .map_err(|e| anyhow::anyhow!("agent slot '{}' load failed: {}", slot_id, e))?;
    let runtime = slot.runtime.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "slot '{}' uses the legacy gateway and has no runtime API",
            slot_id
        )
    })?;
    if runtime.engine != AgentRuntimeEngine::DeepseekHarness {
        anyhow::bail!("slot '{}' is not a DeepSeek Harness runtime", slot_id);
    }
    Ok(runtime.project_dir.clone())
}
