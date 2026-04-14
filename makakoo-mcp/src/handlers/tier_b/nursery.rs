//! Tier-B `nursery_hatch` handler.
//!
//! Registers a new mascot in the `MascotRegistry`, matching the Python
//! `buddy.nursery.hatch` call from `harvey_mcp.py`. Incoming voice is a
//! `{greeting, alert, success, sleeping}` object; species must already
//! exist in the T7 gimmick LEGO catalog (this is not validated here —
//! the registry trusts caller input).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};

use crate::dispatch::{ToolContext, ToolHandler};
use crate::jsonrpc::RpcError;

use makakoo_core::nursery::{Mascot, MascotStatus, MascotVoice};

fn require_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    v.get(key)
        .and_then(|x| x.as_str())
        .ok_or_else(|| RpcError::invalid_params(format!("missing '{key}'")))
}

fn parse_voice(v: &Value) -> Result<MascotVoice, RpcError> {
    let voice = v
        .get("voice")
        .ok_or_else(|| RpcError::invalid_params("missing 'voice'"))?;
    Ok(MascotVoice {
        greeting: require_str(voice, "greeting")?.to_string(),
        alert: require_str(voice, "alert")?.to_string(),
        success: require_str(voice, "success")?.to_string(),
        sleeping: require_str(voice, "sleeping")?.to_string(),
    })
}

fn mascot_to_json(m: &Mascot) -> Value {
    json!({
        "name": m.name,
        "species": m.species,
        "maintainer": m.maintainer,
        "job": m.job,
        "voice": {
            "greeting": m.voice.greeting,
            "alert": m.voice.alert,
            "success": m.voice.success,
            "sleeping": m.voice.sleeping,
        },
        "patrol_interval_hours": m.patrol_interval_hours,
        "created_at": m.created_at.to_rfc3339(),
        "status": format!("{:?}", m.status).to_lowercase(),
    })
}

pub struct NurseryHatchHandler {
    ctx: Arc<ToolContext>,
}

impl NurseryHatchHandler {
    pub fn new(ctx: Arc<ToolContext>) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl ToolHandler for NurseryHatchHandler {
    fn name(&self) -> &str {
        "nursery_hatch"
    }
    fn description(&self) -> &str {
        "Register a new mascot in the nursery. Returns the mascot spec \
         including generated timestamps. Initial status is 'hatching'."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "species": { "type": "string" },
                "maintainer": { "type": "string" },
                "job": { "type": "string" },
                "voice": {
                    "type": "object",
                    "properties": {
                        "greeting": { "type": "string" },
                        "alert": { "type": "string" },
                        "success": { "type": "string" },
                        "sleeping": { "type": "string" }
                    },
                    "required": ["greeting", "alert", "success", "sleeping"]
                },
                "patrol_interval_hours": { "type": "integer", "default": 2 }
            },
            "required": ["name", "species", "maintainer", "job", "voice"]
        })
    }
    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let name = require_str(&params, "name")?.to_string();
        let species = require_str(&params, "species")?.to_string();
        let maintainer = require_str(&params, "maintainer")?.to_string();
        let job = require_str(&params, "job")?.to_string();
        let voice = parse_voice(&params)?;
        let patrol_interval_hours = params
            .get("patrol_interval_hours")
            .and_then(|v| v.as_u64())
            .unwrap_or(2) as u32;

        let mascot = Mascot {
            name,
            species,
            maintainer,
            job,
            voice,
            patrol_interval_hours,
            created_at: Utc::now(),
            status: MascotStatus::Hatching,
        };

        let nursery = self
            .ctx
            .nursery
            .as_ref()
            .ok_or_else(|| RpcError::internal("nursery registry not wired"))?;
        nursery
            .register(mascot.clone())
            .map_err(|e| RpcError::internal(format!("nursery_hatch: {e}")))?;

        Ok(mascot_to_json(&mascot))
    }
}

// ═════════════════════════════════════════════════════════════════════
// Tests
// ═════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use makakoo_core::nursery::MascotRegistry;

    fn ctx() -> (tempfile::TempDir, Arc<ToolContext>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nursery.json");
        let reg = Arc::new(MascotRegistry::load(&path).unwrap());
        let c = ToolContext::empty(dir.path().to_path_buf()).with_nursery(reg);
        (dir, Arc::new(c))
    }

    #[tokio::test]
    async fn hatch_registers_new_mascot() {
        let (_d, ctx) = ctx();
        let h = NurseryHatchHandler::new(ctx.clone());
        let out = h
            .call(json!({
                "name": "Sparky",
                "species": "fox",
                "maintainer": "@contrib",
                "job": "spark catcher",
                "voice": {
                    "greeting": "Zap.",
                    "alert": "Short circuit.",
                    "success": "Conductive.",
                    "sleeping": "Grounded."
                },
                "patrol_interval_hours": 3
            }))
            .await
            .unwrap();
        assert_eq!(out["name"], json!("Sparky"));
        assert_eq!(out["species"], json!("fox"));
        assert_eq!(out["patrol_interval_hours"], json!(3));
        assert_eq!(out["status"], json!("hatching"));
        assert_eq!(ctx.nursery.as_ref().unwrap().get("Sparky").unwrap().name, "Sparky");
    }

    #[tokio::test]
    async fn hatch_rejects_duplicate_name() {
        let (_d, ctx) = ctx();
        let h = NurseryHatchHandler::new(ctx);
        // Olibia is part of the canonical seed.
        let err = h
            .call(json!({
                "name": "Olibia",
                "species": "owl",
                "maintainer": "@someone",
                "job": "dup",
                "voice": {
                    "greeting": "",
                    "alert": "",
                    "success": "",
                    "sleeping": ""
                }
            }))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::INTERNAL_ERROR);
    }
}
