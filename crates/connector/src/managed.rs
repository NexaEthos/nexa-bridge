//! Negotiated local authentication. See docs/wiki/managed-auth.md

use nexa_tunnel_protocol::ReqMeta;
use tokio_tungstenite::tungstenite::http::HeaderValue;

pub const HEADER: &str = "x-nexa-managed-auth";
pub const VERSION: &str = "managed-v1";

pub fn verify_handshake(requested: bool, reply: Option<&HeaderValue>) -> anyhow::Result<()> {
    anyhow::ensure!(
        (requested && reply.is_some_and(|value| value == VERSION))
            || (!requested && reply.is_none()),
        "relay does not support the approved managed authentication contract; update the relay before connecting"
    );
    Ok(())
}

pub fn authorize_local(meta: &mut ReqMeta, local_key: &str) -> anyhow::Result<()> {
    let allowed = (meta.method == "GET" && meta.path == "/v1/models")
        || (meta.method == "POST"
            && matches!(
                meta.path.as_str(),
                "/v1/chat/completions" | "/v1/completions" | "/v1/embeddings" | "/v1/responses"
            ));
    anyhow::ensure!(
        allowed,
        "managed tunnel permits only model inventory and inference"
    );
    anyhow::ensure!(
        !local_key.trim().is_empty(),
        "local bridge credential is missing"
    );
    meta.headers.retain(|(name, _)| {
        !name.eq_ignore_ascii_case("authorization")
            && !name.eq_ignore_ascii_case(HEADER)
            && !name.to_ascii_lowercase().starts_with("x-nexa-internal-")
    });
    meta.headers
        .push(("authorization".into(), format!("Bearer {local_key}")));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_downgrade_or_unsolicited_credential_injection() {
        let supported = HeaderValue::from_static(VERSION);
        assert!(verify_handshake(true, Some(&supported)).is_ok());
        assert!(verify_handshake(false, None).is_ok());
        assert!(verify_handshake(true, None).is_err());
        assert!(verify_handshake(false, Some(&supported)).is_err());
        assert!(verify_handshake(true, Some(&HeaderValue::from_static("unknown"))).is_err());
    }

    #[test]
    fn cloud_authorization_never_reaches_the_loopback_hop() -> anyhow::Result<()> {
        let mut meta = ReqMeta {
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            headers: vec![
                ("Authorization".into(), "Bearer cloud".into()),
                ("AUTHORIZATION".into(), "Bearer forged".into()),
                ("X-Nexa-Internal-Auth".into(), "forged".into()),
                ("content-type".into(), "application/json".into()),
            ],
        };
        authorize_local(&mut meta, "local-only")?;
        assert_eq!(
            meta.headers,
            vec![
                ("content-type".into(), "application/json".into()),
                ("authorization".into(), "Bearer local-only".into())
            ]
        );
        for path in [
            "/_node/logs",
            "//remote.example.com",
            "/v1/models?load=1",
            "/api/v1/models/load",
        ] {
            meta.path = path.into();
            assert!(authorize_local(&mut meta, "local-only").is_err());
        }
        Ok(())
    }
}
