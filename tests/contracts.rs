use chrono::Utc;
use serde_json::json;
use asystant_gateway::{
    config::{ModelConfig, Provider},
    model::{Manifest, Message, TicketClaims, ToolCall, Turn},
    provider::ProviderClient,
    service::GatewayService,
};

fn model() -> ModelConfig {
    ModelConfig {
        id: "test".into(),
        provider: Provider::Openrouter,
        model: "test".into(),
        key_env: "UNUSED".into(),
        wire_api: Default::default(),
        input_micros_per_million: 1_000_000,
        output_micros_per_million: 2_000_000,
        max_input_tokens: 1000,
        max_output_tokens: 100,
    }
}
#[test]
fn reserve_bounds_and_rounding() {
    let m = model();
    assert_eq!(m.reservation().unwrap(), 1200);
    assert_eq!(
        ProviderClient::usage_charge(&m, &json!({"cost":0.0000001})),
        Some(1)
    );
    assert!(
        ProviderClient::usage_charge(&m, &json!({"prompt_tokens":-1,"completion_tokens":0}))
            .is_none()
    );
}
#[test]
fn forbids_tools_without_results_and_forged_roles() {
    let mut turn = Turn {
        registration_id: "r".into(),
        request_id: "id".into(),
        model: "test".into(),
        messages: vec![Message {
            role: "assistant".into(),
            content: "".into(),
            calls: vec![ToolCall {
                id: "t".into(),
                name: "write".into(),
                arguments: json!({}),
            }],
            call_id: "".into(),
        }],
    };
    assert!(GatewayService::validate_turn(&turn).is_err());
    turn.messages.push(Message {
        role: "tool".into(),
        content: "denied".into(),
        calls: vec![],
        call_id: "t".into(),
    });
    assert!(GatewayService::validate_turn(&turn).is_ok());
    turn.messages[0].role = "system".into();
    assert!(GatewayService::validate_turn(&turn).is_err());
}
#[test]
fn rejects_duplicate_tools() {
    let tool = json!({"name":"write","description":"Write","parameters":{"type":"object"}});
    assert!(
        GatewayService::validate_manifest(&Manifest {
            tools: vec![tool.clone(), tool],
            prompts: vec![],
            models: vec!["test".into()]
        })
        .is_err()
    );
}
#[test]
fn tickets_are_short_lived_and_identity_is_unambiguous() {
    let now = Utc::now().timestamp();
    let mut c = TicketClaims {
        iss: "product".into(),
        aud: "asystant-gateway".into(),
        sub: "a/b".into(),
        tenant: "c".into(),
        sid: "s".into(),
        jti: "j".into(),
        iat: now,
        exp: now + 60,
        session_exp: now + 3600,
    };
    assert!(c.validate().is_ok());
    let before = c.identity();
    c.sub = "a".into();
    c.tenant = "b/c".into();
    assert_ne!(before, c.identity());
    c.exp = now + 121;
    assert!(c.validate().is_err());
}
