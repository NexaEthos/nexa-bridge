//! The workspace's presets on a computer and Nexa's local guides. Wiki: docs/wiki/model-selection.md

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How to serve a preset's model on one runtime.
#[derive(Debug, Serialize, Deserialize)]
pub struct RuntimeGuide {
    /// The steps a person follows, in order.
    #[serde(default)]
    pub steps: Vec<String>,
    /// The flags to pass to the runtime.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Nexa's guide to serving a preset on one's own computer.
#[derive(Debug, Serialize, Deserialize)]
pub struct LocalGuide {
    /// The window to load, when the guide sets its own.
    pub context_window: Option<u32>,
    /// Notes for the person.
    pub notes: Option<String>,
    /// Each runtime's steps and flags, keyed by the runtime's kebab-case name.
    #[serde(default)]
    pub runtimes: BTreeMap<String, RuntimeGuide>,
}

/// A Nexa preset the workspace runs on this computer's connection and how to serve it here.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkspacePreset {
    /// The preset's id in Agora.
    pub preset_id: u64,
    /// Its display name.
    pub name: String,
    /// The exact model id requests name; the runtime must serve this id.
    pub model: String,
    /// The window to load.
    pub context_window: Option<u32>,
    /// Whether Nexa certified it.
    pub certified: bool,
    /// Nexa's local guide, when written.
    pub guide: Option<LocalGuide>,
}

impl WorkspacePreset {
    #[doc = "Refuse a preset whose id or name could not be served or shown as is.\n# Errors\nAn empty or padded id, or a name with control characters."]
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.model.is_empty()
                && self.model.trim() == self.model
                && !self.model.chars().any(char::is_control)
                && !self.name.trim().is_empty()
                && !self.name.chars().any(char::is_control),
            "Agora returned an invalid preset guide; refresh or contact your operator"
        );
        Ok(())
    }
}

#[doc = "Read the window LM Studio loaded for the exact id `selected`: the smallest of its loaded instances.\n# Errors\nNo loaded instance of that id, or an invalid reported context."]
pub fn lmstudio_context(value: &Value, selected: &str) -> Result<u32> {
    let models = value.get("models").and_then(Value::as_array).context(
        "LM Studio did not report loaded context; update its server and use Find local models again",
    )?;
    let mut contexts = Vec::new();
    for model in models {
        if let Some(instances) = model.get("loaded_instances").and_then(Value::as_array) {
            for instance in instances {
                if instance.get("id").and_then(Value::as_str) == Some(selected) {
                    let context = instance
                        .get("config")
                        .and_then(|v| v.get("context_length"))
                        .and_then(Value::as_u64)
                        .and_then(|v| u32::try_from(v).ok())
                        .filter(|v| *v > 0 && *v <= 2_000_000)
                        .context("LM Studio reported an invalid loaded context")?;
                    contexts.push(context);
                }
            }
        }
    }
    contexts
        .into_iter()
        .min()
        .context("The selected model is no longer loaded in LM Studio")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_instance_budget_is_not_the_models_advertised_maximum() -> Result<()> {
        let payload = serde_json::json!({"models": [{
            "key": "download-id", "max_context_length": 131_072,
            "loaded_instances": [{"id": "served-id", "config": {"context_length": 32000}}]
        }]});
        assert_eq!(lmstudio_context(&payload, "served-id")?, 32000);
        assert!(lmstudio_context(&payload, "download-id").is_err());
        assert!(lmstudio_context(&serde_json::json!({"models": []}), "served-id").is_err());
        Ok(())
    }

    #[test]
    fn a_guide_names_the_exact_id_to_serve() -> Result<()> {
        let guide: WorkspacePreset = serde_json::from_value(serde_json::json!({
            "preset_id": 43, "name": "Gemma 4 31B", "model": "google/gemma-4-31b-qat",
            "context_window": 131_072, "certified": true,
            "guide": {"context_window": null, "notes": null,
                      "runtimes": {"vllm": {"steps": [], "args": ["--max-model-len", "131072"]}}}
        }))?;
        guide.validate()?;
        assert_eq!(guide.model, "google/gemma-4-31b-qat");
        for model in ["", " padded", "line\nbreak"] {
            let bad = WorkspacePreset {
                model: model.into(),
                ..guide_named("x")
            };
            assert!(bad.validate().is_err());
        }
        Ok(())
    }

    fn guide_named(model: &str) -> WorkspacePreset {
        WorkspacePreset {
            preset_id: 1,
            name: "Model".into(),
            model: model.into(),
            context_window: None,
            certified: false,
            guide: None,
        }
    }
}
