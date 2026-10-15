use super::*;
use crate::oauth::{
    pkce_s256_challenge, OAuthAuthorizationServerMetadata, OAuthAuthorizeResponse,
    OAuthClientRegistrationResponse, OAuthProtectedResourceMetadata, OAuthTokenResponse,
};
use crate::state::MAX_OAUTH_CLIENTS;

pub(super) async fn create_and_login_dev(
    app: &Router,
    admin_token: &str,
    username: &str,
    email: &str,
    password: &str,
) -> (UserPublicProfile, LoginSessionResponse) {
    let invite_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/invites")
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&CreateInviteRequest {
                        target_email: email.to_owned(),
                        assigned_role: EnterpriseRole::Developer,
                        team_ids: vec![],
                        ttl_hours: Some(24),
                    })
                    .expect("serialize invite"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(invite_resp.status(), StatusCode::CREATED);
    let issued_invite: IssuedInviteLink = serde_json::from_slice(
        &invite_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse invite");

    let redeem_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/invites/redeem")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "raw_token": issued_invite.raw_token,
                        "username": username,
                        "password": password
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let dev_profile: UserPublicProfile = serde_json::from_slice(
        &redeem_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse dev_profile");

    let dev_login_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "identifier": username,
                        "password": password
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    let dev_login: LoginSessionResponse = serde_json::from_slice(
        &dev_login_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse dev_login");
    (dev_profile, dev_login)
}

#[tokio::test]
async fn oauth_pkce_s256_mcp_http_lifecycle_and_deactivation_gate() {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    init_temp_repo(&repo);
    let (app, admin_token) = test_app_at(repo.clone(), true);

    let unauth_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauth_resp.status(), StatusCode::UNAUTHORIZED);
    let www_auth = unauth_resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        www_auth.contains("resource_metadata=\"/.well-known/oauth-protected-resource\""),
        "unexpected WWW-Authenticate: {www_auth}"
    );

    let pr_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/oauth-protected-resource")
                .header("host", "127.0.0.1:3000")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(pr_resp.status(), StatusCode::OK);
    let pr_meta: OAuthProtectedResourceMetadata = serde_json::from_slice(
        &pr_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse pr_meta");
    assert_eq!(pr_meta.resource, "http://127.0.0.1:3000/mcp");

    let as_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/oauth-authorization-server")
                .header("host", "127.0.0.1:3000")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(as_resp.status(), StatusCode::OK);
    let as_meta: OAuthAuthorizationServerMetadata = serde_json::from_slice(
        &as_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse as_meta");
    assert_eq!(as_meta.code_challenge_methods_supported, vec!["S256"]);

    for bad_uri in [
        "javascript:alert(1)",
        "http://localhost.attacker.com/cb",
        "http://127.0.0.1@attacker.com/cb",
        "http://127.0.0.1:8080/cb#frag",
        "https://",
    ] {
        let bad_reg_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "client_name": "Evil Client",
                            "redirect_uris": [bad_uri]
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            bad_reg_resp.status(),
            StatusCode::BAD_REQUEST,
            "bad redirect_uri must be rejected: {bad_uri}"
        );
    }

    for i in 0..(MAX_OAUTH_CLIENTS + 4) {
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/oauth/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "client_name": format!("Temp Client {i}"),
                            "redirect_uris": ["http://127.0.0.1:54321/oauth/callback"]
                        })
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
    }

    let reg_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "client_name": "Claude Code MCP Client",
                        "redirect_uris": ["http://127.0.0.1:54321/oauth/callback"]
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(reg_resp.status(), StatusCode::CREATED);
    let reg_client: OAuthClientRegistrationResponse = serde_json::from_slice(
        &reg_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse reg_client");

    let (dev_profile, dev_login) = create_and_login_dev(
        &app,
        &admin_token,
        "mcpdev",
        "mcpdev@company.internal",
        "McpDevPassword!2026",
    )
    .await;

    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk-gitust-mcp-2026";
    let challenge = pkce_s256_challenge(verifier);

    let builtin_authz_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", dev_login.raw_session_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "client_id": "gitust-mcp-cli",
                        "redirect_uri": "http://127.0.0.1/callback",
                        "response_type": "code",
                        "code_challenge": challenge,
                        "code_challenge_method": "S256"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(builtin_authz_resp.status(), StatusCode::OK);

    let authz_resp_1 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", dev_login.raw_session_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "client_id": reg_client.client_id,
                        "redirect_uri": "http://127.0.0.1:54321/oauth/callback",
                        "response_type": "code",
                        "code_challenge": challenge,
                        "code_challenge_method": "S256",
                        "state": "xyz-state-1"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(authz_resp_1.status(), StatusCode::OK);
    let authz_1: OAuthAuthorizeResponse = serde_json::from_slice(
        &authz_resp_1
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse authz_1");

    let bad_tok_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "grant_type": "authorization_code",
                        "code": authz_1.code,
                        "client_id": reg_client.client_id,
                        "redirect_uri": "http://127.0.0.1:54321/oauth/callback",
                        "code_verifier": "wrong_verifier_that_is_at_least_43_chars_long_1234567890"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(bad_tok_resp.status(), StatusCode::BAD_REQUEST);

    let authz_resp_2 = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/authorize")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", dev_login.raw_session_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "client_id": reg_client.client_id,
                        "redirect_uri": "http://127.0.0.1:54321/oauth/callback",
                        "response_type": "code",
                        "code_challenge": challenge,
                        "code_challenge_method": "S256"
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(authz_resp_2.status(), StatusCode::OK);
    let authz_2: OAuthAuthorizeResponse = serde_json::from_slice(
        &authz_resp_2
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse authz_2");

    let form_body = format!(
        "grant_type=authorization_code&code={}&client_id={}&redirect_uri={}&code_verifier={}",
        authz_2.code,
        reg_client.client_id,
        "http%3A%2F%2F127.0.0.1%3A54321%2Foauth%2Fcallback",
        verifier
    );
    let tok_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form_body.clone()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(tok_resp.status(), StatusCode::OK);
    let oauth_token: OAuthTokenResponse = serde_json::from_slice(
        &tok_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse oauth_token");
    assert_eq!(oauth_token.token_type, "Bearer");
    assert!(oauth_token.access_token.starts_with("gitust_ses_"));

    let replay_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form_body))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(replay_resp.status(), StatusCode::BAD_REQUEST);

    let mcp_init_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", oauth_token.access_token),
                    )
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
    assert_eq!(mcp_init_resp.status(), StatusCode::OK);

    let mcp_notif_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", oauth_token.access_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(mcp_notif_resp.status(), StatusCode::ACCEPTED);

    let mcp_spawn_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/mcp")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", oauth_token.access_token),
                    )
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"gitust_worktree_spawn","arguments":{"agent":"claude_code","prompt_summary":"HTTP OAuth MCP test"}}}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
    assert_eq!(mcp_spawn_resp.status(), StatusCode::OK);
    let spawn_rpc: serde_json::Value = serde_json::from_slice(
        &mcp_spawn_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("parse spawn_rpc");
    let spawn_payload: serde_json::Value = serde_json::from_str(
        spawn_rpc["result"]["content"][0]["text"]
            .as_str()
            .expect("text"),
    )
    .expect("parse spawn_payload");
    assert!(
        spawn_payload.get("worktree_dir").is_none(),
        "HTTP MCP response must not leak host filesystem worktree_dir"
    );
    let wt_id = spawn_payload["worktree_id"].as_str().expect("wt_id");

    let mcp_discard_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", oauth_token.access_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 3,
                        "method": "tools/call",
                        "params": {
                            "name": "gitust_worktree_discard",
                            "arguments": { "worktree_id": wt_id }
                        }
                    })
                    .to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(mcp_discard_resp.status(), StatusCode::OK);

    let deact_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/v1/admin/users/{}/deactivate",
                    dev_profile.id.0
                ))
                .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(deact_resp.status(), StatusCode::OK);

    let revoked_mcp_resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", oauth_token.access_token),
                )
                .header("content-type", "application/json")
                .body(Body::from(r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(revoked_mcp_resp.status(), StatusCode::FORBIDDEN);
}
