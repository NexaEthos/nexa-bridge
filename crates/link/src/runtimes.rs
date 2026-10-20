//! Local model runtime detection. Wiki: docs/wiki/desktop.md#runtime-flavors

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

const CANDIDATES: [&str; 5] = [
    "http://127.0.0.1:11434",
    "http://127.0.0.1:1234",
    "http://127.0.0.1:8080",
    "http://127.0.0.1:8000",
    "http://127.0.0.1:30000",
];

/// The kind of model server answering at an address; its name is also the local guide's runtime key.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flavor {
    /// Ollama.
    Ollama,
    /// LM Studio's local server.
    LmStudio,
    /// llama.cpp's server.
    LlamaCpp,
    /// vLLM.
    Vllm,
    /// Any other OpenAI-compatible server (`SGLang` among them).
    OpenAiCompatible,
}

/// How a runtime handles requests arriving together.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Concurrency {
    /// Batches requests.
    Parallel,
    /// Serves one at a time by default.
    Serialized,
    /// Not known from the runtime's kind.
    Unknown,
}

impl Flavor {
    /// The runtime's display name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::LmStudio => "LM Studio",
            Self::LlamaCpp => "llama.cpp",
            Self::Vllm => "vLLM",
            Self::OpenAiCompatible => "OpenAI-compatible server",
        }
    }

    /// How this kind of runtime handles concurrent requests.
    #[must_use]
    pub const fn concurrency(self) -> Concurrency {
        match self {
            Self::Vllm => Concurrency::Parallel,
            Self::Ollama | Self::LlamaCpp => Concurrency::Serialized,
            Self::LmStudio | Self::OpenAiCompatible => Concurrency::Unknown,
        }
    }

    /// A sentence of advice for this runtime, if any.
    #[must_use]
    pub const fn note(self) -> Option<&'static str> {
        match self {
            Self::Ollama => Some(
                "Ollama serves few requests in parallel by default. Raise OLLAMA_NUM_PARALLEL \
                 before pointing several agents at it, or they will queue.",
            ),
            Self::LlamaCpp => Some(
                "llama.cpp serves one request at a time unless it was started with --parallel.",
            ),
            Self::LmStudio => Some(
                "Check LM Studio's server settings for its concurrent-request limit before \
                 running several agents.",
            ),
            Self::Vllm => Some("vLLM batches requests continuously — good for multi-agent work."),
            Self::OpenAiCompatible => None,
        }
    }
}

/// A model server found at an address, with the models it reports.
#[derive(Clone, Debug, Serialize)]
pub struct Runtime {
    /// The server's base address, without `/v1`.
    pub url: String,
    /// What kind of server it is.
    pub flavor: Flavor,
    /// Its display name.
    pub label: String,
    /// The model ids it reports.
    pub models: Vec<String>,
    /// The ids loaded now, when the server tells loaded from downloaded.
    pub loaded_models: Option<Vec<String>>,
    /// How it handles concurrent requests.
    pub concurrency: Concurrency,
    /// Advice for this runtime, if any.
    pub note: Option<&'static str>,
}

/// Turn what a person typed into a base address: a scheme added, `/v1` and trailing slashes removed.
#[must_use]
pub fn normalize_url(input: &str) -> Option<String> {
    let raw = input.trim().trim_end_matches('/');
    if raw.is_empty() {
        return None;
    }
    let with_scheme = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    Some(
        with_scheme
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string(),
    )
}

fn model_names(value: &Value, collection: &str, name: &str) -> Option<Vec<String>> {
    if value.get("error").is_some() {
        return None;
    }
    value
        .get(collection)?
        .as_array()?
        .iter()
        .map(|model| {
            model
                .get(name)?
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .map(str::to_string)
        })
        .collect()
}

fn looks_like_vllm(v: &Value) -> bool {
    v.get("data")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|m| m.get("max_model_len").is_some()))
}

fn lmstudio_loaded_models(value: &Value) -> Option<Vec<String>> {
    let names = model_names(value, "data", "id")?;
    let mut loaded = Vec::new();
    for (model, name) in value.get("data")?.as_array()?.iter().zip(names) {
        match model.get("state")?.as_str()? {
            "loaded" => loaded.push(name),
            "not-loaded" => {}
            _ => return None,
        }
    }
    Some(loaded)
}

fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

async fn get_json(client: &reqwest::Client, url: String) -> Option<Value> {
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let value = resp.json::<Value>().await.ok()?;
    if value.get("error").is_some() {
        tracing::debug!("runtime probe rejected a JSON error response");
        return None;
    }
    Some(value)
}

/// Ask one address what model server answers there.
pub async fn probe(client: &reqwest::Client, base: &str) -> Option<Runtime> {
    let openai = get_json(client, format!("{base}/v1/models")).await;

    let (tags, lmstudio, props) = tokio::join!(
        get_json(client, format!("{base}/api/tags")),
        get_json(client, format!("{base}/api/v0/models")),
        get_json(client, format!("{base}/props")),
    );

    let (flavor, models) = classify(
        openai.as_ref(),
        tags.as_ref(),
        lmstudio.as_ref(),
        props.as_ref(),
    )?;

    Some(Runtime {
        url: base.to_string(),
        flavor,
        label: flavor.label().to_string(),
        models,
        loaded_models: if flavor == Flavor::LmStudio {
            lmstudio.as_ref().and_then(lmstudio_loaded_models)
        } else {
            None
        },
        concurrency: flavor.concurrency(),
        note: flavor.note(),
    })
}

fn classify(
    openai: Option<&Value>,
    tags: Option<&Value>,
    lmstudio: Option<&Value>,
    props: Option<&Value>,
) -> Option<(Flavor, Vec<String>)> {
    if let Some(models) = tags.and_then(|value| model_names(value, "models", "name")) {
        return Some((Flavor::Ollama, models));
    }
    let openai = openai?;
    let models = model_names(openai, "data", "id")?;
    let flavor = if lmstudio
        .and_then(|value| model_names(value, "data", "id"))
        .is_some()
    {
        Flavor::LmStudio
    } else if props.is_some_and(|value| {
        value.get("error").is_none()
            && value
                .get("default_generation_settings")
                .is_some_and(Value::is_object)
    }) {
        Flavor::LlamaCpp
    } else if looks_like_vllm(openai) {
        Flavor::Vllm
    } else {
        Flavor::OpenAiCompatible
    };
    Some((flavor, models))
}

/// Probe the usual loopback addresses of local model servers.
pub async fn detect() -> Vec<Runtime> {
    let Ok(client) = client() else {
        return Vec::new();
    };
    let found =
        futures_util::future::join_all(CANDIDATES.iter().map(|base| probe(&client, base))).await;
    found.into_iter().flatten().collect()
}

#[doc = "Read the window LM Studio loaded for the exact id `model` at `upstream` (its native models list).\n# Errors\nLM Studio unreachable, or no readable loaded context for that id."]
pub async fn lmstudio_loaded_context(upstream: &str, model: &str) -> anyhow::Result<u32> {
    let value = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(std::time::Duration::from_secs(5))
        .build()?
        .get(format!("{upstream}/api/v1/models"))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await
        .map_err(|_| {
            anyhow::anyhow!("Cannot read the context LM Studio loaded for the selected model")
        })?;
    crate::guides::lmstudio_context(&value, model)
}

#[doc = "Probe one address a person entered.\n# Errors\nAn empty address, or nothing answering there."]
pub async fn probe_one(url: &str) -> Result<Runtime, String> {
    let base = normalize_url(url).ok_or("enter an address first")?;
    let client = client().map_err(|e| e.to_string())?;
    probe(&client, &base)
        .await
        .ok_or_else(|| format!("nothing answered at {base}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lmstudio_distinguishes_loaded_models_from_downloaded_inventory() {
        let listing = json!({"data": [
            {"id": "lfm-4bit", "state": "loaded"},
            {"id": "lfm-bf16", "state": "not-loaded"},
            {"id": "embedding", "state": "loaded"}
        ]});
        assert_eq!(
            lmstudio_loaded_models(&listing),
            Some(vec!["lfm-4bit".into(), "embedding".into()])
        );
        assert_eq!(
            lmstudio_loaded_models(&json!({"data": [
                {"id": "lfm-bf16", "state": "not-loaded"}
            ]})),
            Some(vec![])
        );
        assert_eq!(lmstudio_loaded_models(&json!({"data": []})), Some(vec![]));
    }

    #[test]
    fn lmstudio_missing_or_invalid_load_state_is_unknown_not_empty() {
        for listing in [
            json!({"error": "unavailable", "data": []}),
            json!({"data": [{"id": "lfm"}]}),
            json!({"data": [{"id": "lfm", "state": "loading"}]}),
            json!({"data": [{"id": "lfm", "state": null}]}),
            json!({"data": [{"id": "lfm", "state": "loaded"}, {"id": "other"}]}),
            json!({"data": [{"state": "loaded"}]}),
        ] {
            assert!(lmstudio_loaded_models(&listing).is_none());
        }
    }

    #[test]
    fn normalize_accepts_what_users_actually_paste() {
        assert_eq!(
            normalize_url("127.0.0.1:11434").as_deref(),
            Some("http://127.0.0.1:11434")
        );
        assert_eq!(
            normalize_url(" http://127.0.0.1:1234/ ").as_deref(),
            Some("http://127.0.0.1:1234")
        );
        assert_eq!(
            normalize_url("http://127.0.0.1:8000/v1").as_deref(),
            Some("http://127.0.0.1:8000")
        );
        assert_eq!(
            normalize_url("https://box.example.com").as_deref(),
            Some("https://box.example.com")
        );
        assert_eq!(normalize_url("   "), None);
    }

    #[test]
    fn parses_openai_and_ollama_listings() {
        let openai = json!({"object": "list", "data": [{"id": "a"}, {"id": "b"}]});
        assert_eq!(
            model_names(&openai, "data", "id"),
            Some(vec!["a".into(), "b".into()])
        );
        let tags = json!({"models": [{"name": "llama3:8b"}, {"name": "qwen:7b"}]});
        assert_eq!(
            model_names(&tags, "models", "name"),
            Some(vec!["llama3:8b".into(), "qwen:7b".into()])
        );
        assert!(model_names(&json!({"unexpected": true}), "data", "id").is_none());
        assert!(model_names(&json!([]), "models", "name").is_none());
        assert!(model_names(&json!({"data": [{"no_id": 1}]}), "data", "id").is_none());
        assert!(model_names(&json!({"data": [{"id": " "}]}), "data", "id").is_none());
        assert_eq!(
            model_names(&json!({"data": []}), "data", "id"),
            Some(vec![])
        );
    }

    #[test]
    fn vllm_is_recognised_by_its_context_length_field() {
        assert!(looks_like_vllm(
            &json!({"data": [{"id": "m", "max_model_len": 65536}]})
        ));
        assert!(!looks_like_vllm(&json!({"data": [{"id": "m"}]})));
    }

    #[test]
    fn concurrency_matches_the_flavor() {
        assert_eq!(Flavor::Vllm.concurrency(), Concurrency::Parallel);
        assert_eq!(Flavor::Ollama.concurrency(), Concurrency::Serialized);
        assert_eq!(Flavor::LmStudio.concurrency(), Concurrency::Unknown);
        for f in [Flavor::Ollama, Flavor::LlamaCpp, Flavor::LmStudio] {
            assert!(f.note().is_some(), "{f:?} needs a note");
        }
    }

    #[test]
    fn identifies_ollama_from_a_valid_native_listing() {
        let tags = json!({"models": [{"name": "llama3:8b"}]});
        assert_eq!(
            classify(None, Some(&tags), None, None),
            Some((Flavor::Ollama, vec!["llama3:8b".into()]))
        );
    }

    #[test]
    fn distinguishes_vllm_from_a_plain_openai_listing() {
        let vllm = json!({"data": [{"id": "org/model", "max_model_len": 65536}]});
        assert_eq!(
            classify(Some(&vllm), None, None, None).map(|result| result.0),
            Some(Flavor::Vllm)
        );
        let plain = json!({"data": [{"id": "some-model"}]});
        assert_eq!(
            classify(Some(&plain), None, None, None),
            Some((Flavor::OpenAiCompatible, vec!["some-model".into()]))
        );
    }

    #[test]
    fn error_payloads_do_not_misidentify_lmstudio_or_llamacpp() {
        let listing = json!({"data": [{"id": "m"}]});
        let error = json!({"error": "Unexpected endpoint or method. (GET /api/tags)"});
        let lms = json!({"data": []});
        assert_eq!(
            classify(Some(&listing), Some(&error), Some(&lms), Some(&error)),
            Some((Flavor::LmStudio, vec!["m".into()]))
        );
        let props = json!({"default_generation_settings": {}});
        assert_eq!(
            classify(Some(&listing), Some(&error), Some(&error), Some(&props))
                .map(|result| result.0),
            Some(Flavor::LlamaCpp)
        );
        assert_eq!(
            classify(Some(&listing), Some(&error), Some(&error), Some(&error))
                .map(|result| result.0),
            Some(Flavor::OpenAiCompatible)
        );
    }

    #[test]
    fn rejects_errors_and_malformed_runtime_responses() {
        let error = json!({"error": "failed", "data": [], "models": []});
        assert!(classify(Some(&error), Some(&error), Some(&error), Some(&error)).is_none());
        assert!(classify(None, None, None, None).is_none());
        let malformed = json!({"data": {}, "models": {}});
        assert!(classify(Some(&malformed), Some(&malformed), None, None).is_none());
        let listing = json!({"data": []});
        assert_eq!(
            classify(Some(&listing), None, Some(&malformed), Some(&malformed))
                .map(|result| result.0),
            Some(Flavor::OpenAiCompatible)
        );
    }
}
