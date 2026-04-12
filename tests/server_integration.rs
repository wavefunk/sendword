use std::sync::Arc;

use sendword::config::{AppConfig, ExecutorConfig, HookAuthConfig, HmacAlgorithm, HookConfig};
use sendword::db::Db;
use sendword::models::trigger_attempt::{self, TriggerAttemptStatus};
use sendword::payload::{FieldType, PayloadField, PayloadSchema};
use sendword::server::AppState;
use sendword::templates::Templates;

async fn test_state(config: AppConfig) -> Arc<AppState> {
    let db = Db::new_in_memory().await.expect("in-memory db");
    db.migrate().await.expect("migration");
    let templates = Templates::new(Templates::default_dir());
    AppState::new(config, "sendword.toml", db, templates)
}

async fn spawn_server(state: Arc<AppState>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let url = format!("http://{addr}");

    tokio::spawn(async move {
        axum::serve(listener, sendword::server::into_service(state))
            .await
            .expect("server");
    });

    url
}

async fn spawn_test_server() -> String {
    let state = test_state(AppConfig::default()).await;
    spawn_server(state).await
}

async fn create_test_session(state: &Arc<AppState>) -> String {
    use sendword::models::{session, user};
    use std::time::Duration;

    let pool = state.db.pool();
    let u = user::create(pool, "testadmin", "testpass123")
        .await
        .expect("create test user");
    let sess = session::create(pool, &u.id, Duration::from_secs(3600))
        .await
        .expect("create test session");
    sess.id
}

async fn spawn_authed_server(config: AppConfig) -> (String, String) {
    let state = test_state(config).await;
    let token = create_test_session(&state).await;
    let url = spawn_server(state).await;
    (url, token)
}

fn client_no_redirect() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn healthz_returns_ok() {
    let url = spawn_test_server().await;
    let resp = reqwest::get(format!("{url}/healthz")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn healthz_returns_json_content_type() {
    let url = spawn_test_server().await;
    let resp = reqwest::get(format!("{url}/healthz")).await.unwrap();
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("application/json"));
}

#[tokio::test]
async fn dashboard_returns_html() {
    let (url, token) = spawn_authed_server(AppConfig::default()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("sendword"));
    assert!(body.contains("Hooks"));
}

#[tokio::test]
async fn nonexistent_route_returns_404() {
    let url = spawn_test_server().await;
    let resp = reqwest::get(format!("{url}/nonexistent")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn stub_routes_return_not_implemented() {
    let (url, token) = spawn_authed_server(AppConfig::default()).await;
    let client = reqwest::Client::new();

    // trigger_hook returns 404 for non-existent/disabled hooks (unprotected)
    let resp = client.post(format!("{url}/hook/test-hook")).send().await.unwrap();
    assert_eq!(resp.status(), 404);

    // hook_detail returns 404 for non-existent hooks (protected)
    let resp = client
        .get(format!("{url}/hooks/test-hook"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // execution_detail returns 404 for non-existent executions (protected)
    let resp = client
        .get(format!("{url}/executions/some-id"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // replay returns 404 for non-existent executions (protected)
    let resp = client
        .post(format!("{url}/executions/some-id/replay"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

/// Verifies that routes enforce their declared HTTP methods.
/// A GET to a POST-only route (e.g. /hook/:slug) must return 405, not 501.
/// This catches accidental method changes in route definitions that could
/// allow unintended hook triggers.
#[tokio::test]
async fn wrong_http_method_returns_405() {
    let url = spawn_test_server().await;
    let client = reqwest::Client::new();

    // GET to a POST-only route
    let resp = client
        .get(format!("{url}/hook/test-hook"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);

    // POST to a GET-only route
    let resp = client
        .post(format!("{url}/hooks/test-hook"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 405);

    // POST to the dashboard (GET-only)
    let resp = client.post(format!("{url}/")).send().await.unwrap();
    assert_eq!(resp.status(), 405);

    // POST to healthz (GET-only)
    let resp = client.post(format!("{url}/healthz")).send().await.unwrap();
    assert_eq!(resp.status(), 405);
}

/// Verifies that the dashboard renders hooks from config.
/// The default config has no hooks; this test configures hooks and verifies
/// they appear in the rendered HTML, exercising the config-to-template pipeline.
#[tokio::test]
async fn dashboard_renders_configured_hooks() {
    use sendword::config::{ExecutorConfig, HookConfig};
    use std::collections::HashMap;

    let config = AppConfig {
        hooks: vec![
            HookConfig {
                name: "Deploy App".into(),
                slug: "deploy-app".into(),
                description: "Deploys the application".into(),
                enabled: true,
                auth: None,
                executor: ExecutorConfig::Shell {
                    command: "make deploy".into(),
                },
                env: HashMap::new(),
                cwd: None,
                timeout: None,
                retries: None,
                rate_limit: None,
                payload: None,
                trigger_rules: None,
            },
            HookConfig {
                name: "Run Tests".into(),
                slug: "run-tests".into(),
                description: String::new(),
                enabled: false,
                auth: None,
                executor: ExecutorConfig::Shell {
                    command: "make test".into(),
                },
                env: HashMap::new(),
                cwd: None,
                timeout: None,
                retries: None,
                rate_limit: None,
                payload: None,
                trigger_rules: None,
            },
        ],
        ..AppConfig::default()
    };

    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // Both hook names should appear
    assert!(body.contains("Deploy App"), "dashboard should show hook name");
    assert!(body.contains("Run Tests"), "dashboard should show second hook name");

    // Slugs should appear as URL paths
    assert!(
        body.contains("/hook/deploy-app"),
        "dashboard should show hook URL path"
    );
    assert!(
        body.contains("/hook/run-tests"),
        "dashboard should show second hook URL path"
    );

    // Enabled/disabled status should be rendered
    assert!(body.contains("enabled"), "dashboard should show enabled status");
    assert!(body.contains("disabled"), "dashboard should show disabled status");
}

#[tokio::test]
async fn dashboard_shows_last_execution_status() {
    use sendword::config::{ExecutorConfig, HookConfig};
    use sendword::models::execution::{self, NewExecution};
    use sendword::models::ExecutionStatus;
    use std::collections::HashMap;

    let config = AppConfig {
        hooks: vec![HookConfig {
            name: "Test Hook".into(),
            slug: "test-hook".into(),
            description: "A test hook".into(),
            enabled: true,
            auth: None,
            executor: ExecutorConfig::Shell {
                command: "echo ok".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
            payload: None,
            trigger_rules: None,
        }],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;

    // Create an execution and mark it as success
    let exec = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "data/logs/test",
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();
    execution::mark_running(state.db.pool(), &exec.id)
        .await
        .unwrap();
    execution::mark_completed(state.db.pool(), &exec.id, ExecutionStatus::Success, Some(0))
        .await
        .unwrap();

    let url = spawn_server(state).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("Test Hook"), "should show hook name");
    assert!(body.contains("success"), "should show last execution status");
    assert!(
        body.contains("/hooks/test-hook"),
        "should link to hook detail"
    );
}

#[tokio::test]
async fn dashboard_shows_no_executions_for_new_hook() {
    use sendword::config::{ExecutorConfig, HookConfig};
    use std::collections::HashMap;

    let config = AppConfig {
        hooks: vec![HookConfig {
            name: "Fresh Hook".into(),
            slug: "fresh-hook".into(),
            description: String::new(),
            enabled: true,
            auth: None,
            executor: ExecutorConfig::Shell {
                command: "echo hi".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
            payload: None,
            trigger_rules: None,
        }],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();

    assert!(body.contains("Fresh Hook"));
    assert!(body.contains("No executions yet"));
}

// --- Hook detail page tests ---

fn make_test_hook(name: &str, slug: &str, command: &str) -> sendword::config::HookConfig {
    use sendword::config::{ExecutorConfig, HookConfig};
    use std::collections::HashMap;

    HookConfig {
        name: name.into(),
        slug: slug.into(),
        description: "A test hook for integration tests".into(),
        enabled: true,
        auth: None,
        executor: ExecutorConfig::Shell {
            command: command.into(),
        },
        env: HashMap::from([("APP_ENV".into(), "test".into())]),
        cwd: Some("/tmp".into()),
        timeout: None,
        retries: None,
        rate_limit: None,
        payload: None,
        trigger_rules: None,
    }
}

#[tokio::test]
async fn hook_detail_returns_404_for_unknown_slug() {
    let (url, token) = spawn_authed_server(AppConfig::default()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/hooks/nonexistent"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn hook_detail_renders_hook_config() {
    let config = AppConfig {
        hooks: vec![make_test_hook("Deploy App", "deploy-app", "make deploy")],
        ..AppConfig::default()
    };

    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/hooks/deploy-app"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // Hook name and slug
    assert!(body.contains("Deploy App"), "should show hook name");
    assert!(body.contains("/hook/deploy-app"), "should show trigger URL");

    // Description
    assert!(
        body.contains("A test hook for integration tests"),
        "should show description"
    );

    // Enabled status
    assert!(body.contains("enabled"), "should show enabled status");

    // Executor config
    assert!(body.contains("shell"), "should show executor type");
    assert!(body.contains("make deploy"), "should show command");
    // MiniJinja HTML-escapes "/" to "&#x2f;", so check for the escaped form
    assert!(
        body.contains("&#x2f;tmp") || body.contains("/tmp"),
        "should show working directory"
    );
    assert!(body.contains("APP_ENV"), "should show env var name");

    // Timeout (should show the default 30s)
    assert!(body.contains("30"), "should show timeout");

    // Back to dashboard link
    assert!(body.contains("Back to dashboard"), "should have back link");

    // Auth section is now always shown (with "none" for public hooks)
    assert!(body.contains("Authentication"), "should show auth section");
    assert!(body.contains("none"), "should show none auth mode for public hook");

    // Payload section is guarded by `is defined` and should be absent
    // when the handler doesn't pass payload_fields in the context.
    assert!(
        !body.contains("Payload Schema"),
        "should not show payload section when payload_fields is not in context"
    );
}

#[tokio::test]
async fn hook_detail_shows_disabled_hook() {
    use sendword::config::{ExecutorConfig, HookConfig};
    use std::collections::HashMap;

    let config = AppConfig {
        hooks: vec![HookConfig {
            name: "Disabled Hook".into(),
            slug: "disabled-hook".into(),
            description: String::new(),
            enabled: false,
            auth: None,
            executor: ExecutorConfig::Shell {
                command: "echo nope".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
            payload: None,
            trigger_rules: None,
        }],
        ..AppConfig::default()
    };

    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/hooks/disabled-hook"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("disabled"), "should show disabled status");
}

#[tokio::test]
async fn hook_detail_shows_execution_history() {
    use sendword::models::execution::{self, NewExecution};
    use sendword::models::ExecutionStatus;

    let config = AppConfig {
        hooks: vec![make_test_hook("Test Hook", "test-hook", "echo ok")],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;

    // Create executions with different statuses
    let exec1 = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "data/logs/test1",
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();
    execution::mark_running(state.db.pool(), &exec1.id)
        .await
        .unwrap();
    execution::mark_completed(state.db.pool(), &exec1.id, ExecutionStatus::Success, Some(0))
        .await
        .unwrap();

    let exec2 = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "data/logs/test2",
            trigger_source: "10.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();
    execution::mark_running(state.db.pool(), &exec2.id)
        .await
        .unwrap();
    execution::mark_completed(state.db.pool(), &exec2.id, ExecutionStatus::Failed, Some(1))
        .await
        .unwrap();

    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    // Test the full page
    let resp = client
        .get(format!("{url}/hooks/test-hook"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("2 total"), "should show total count");
    assert!(
        body.contains("Execution History"),
        "should have execution history section"
    );

    // Test the HTMX partial endpoint
    let resp = client
        .get(format!("{url}/hooks/test-hook/executions?page=1"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let partial = resp.text().await.unwrap();

    // Both executions should appear in the partial
    assert!(partial.contains("success"), "should show success status");
    assert!(partial.contains("failed"), "should show failed status");

    // Should link to execution detail
    assert!(
        partial.contains("/executions/"),
        "should link to execution detail"
    );
}

#[tokio::test]
async fn hook_detail_execution_list_returns_404_for_unknown_hook() {
    let (url, token) = spawn_authed_server(AppConfig::default()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/hooks/nonexistent/executions"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn hook_detail_no_executions_shows_empty_message() {
    let config = AppConfig {
        hooks: vec![make_test_hook("Empty Hook", "empty-hook", "echo hi")],
        ..AppConfig::default()
    };

    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();

    // The HTMX partial for a hook with no executions
    let resp = client
        .get(format!("{url}/hooks/empty-hook/executions?page=1"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(
        body.contains("No executions yet"),
        "should show empty state message"
    );
}

// --- Replay handler tests ---

#[tokio::test]
async fn replay_returns_404_for_nonexistent_execution() {
    let (url, token) = spawn_authed_server(AppConfig::default()).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{url}/executions/nonexistent-id/replay"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn replay_returns_404_when_hook_no_longer_exists() {
    use sendword::models::execution::{self, NewExecution};

    // Create state with no hooks configured
    let state = test_state(AppConfig::default()).await;
    let token = create_test_session(&state).await;

    // Insert an execution for a hook slug that doesn't exist in config
    let exec = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "deleted-hook",
            log_path: "data/logs/deleted",
            trigger_source: "127.0.0.1",
            request_payload: r#"{"key": "value"}"#,
            retry_of: None,
        },
    )
    .await
    .unwrap();

    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{url}/executions/{}/replay", exec.id))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn replay_creates_new_execution_linked_to_original() {
    use sendword::config::{ExecutorConfig, HookConfig};
    use sendword::models::execution::{self, NewExecution};
    use sendword::models::ExecutionStatus;
    use std::collections::HashMap;

    let config = AppConfig {
        hooks: vec![HookConfig {
            name: "Echo Hook".into(),
            slug: "echo-hook".into(),
            description: String::new(),
            enabled: true,
            auth: None,
            executor: ExecutorConfig::Shell {
                command: "echo replayed".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
            payload: None,
            trigger_rules: None,
        }],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;
    let pool = state.db.pool().clone();

    // Create an original execution and mark it completed
    let original = execution::create(
        &pool,
        &NewExecution {
            id: None,
            hook_slug: "echo-hook",
            log_path: "data/logs/original",
            trigger_source: "10.0.0.1",
            request_payload: r#"{"action": "deploy"}"#,
            retry_of: None,
        },
    )
    .await
    .unwrap();
    execution::mark_running(&pool, &original.id).await.unwrap();
    execution::mark_completed(&pool, &original.id, ExecutionStatus::Success, Some(0))
        .await
        .unwrap();

    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    // Replay the execution
    let resp = client
        .post(format!("{url}/executions/{}/replay", original.id))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let new_id = body["execution_id"].as_str().expect("execution_id in response");
    assert!(!new_id.is_empty(), "new execution_id should be non-empty");
    assert_ne!(new_id, original.id, "replay should create a new execution");

    // Verify the new execution in the DB
    let replay_exec = execution::get_by_id(&pool, new_id).await.unwrap();
    assert_eq!(replay_exec.hook_slug, "echo-hook");
    assert_eq!(replay_exec.request_payload, r#"{"action": "deploy"}"#);
    assert_eq!(replay_exec.trigger_source, "10.0.0.1");
    assert_eq!(replay_exec.retry_of.as_deref(), Some(original.id.as_str()));
}

#[tokio::test]
async fn replay_spawns_executor_and_runs_command() {
    use sendword::config::{ExecutorConfig, HookConfig};
    use sendword::models::execution::{self, NewExecution};
    use sendword::models::ExecutionStatus;
    use std::collections::HashMap;

    let tmp = tempfile::TempDir::new().expect("temp dir");
    let logs_dir = tmp.path().to_str().expect("utf-8 path");

    let config = AppConfig {
        logs: sendword::config::LogsConfig {
            dir: logs_dir.into(),
        },
        hooks: vec![HookConfig {
            name: "Echo Hook".into(),
            slug: "echo-hook".into(),
            description: String::new(),
            enabled: true,
            auth: None,
            executor: ExecutorConfig::Shell {
                command: "echo replayed-output".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
            payload: None,
            trigger_rules: None,
        }],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;
    let pool = state.db.pool().clone();

    // Create the original execution
    let original = execution::create(
        &pool,
        &NewExecution {
            id: None,
            hook_slug: "echo-hook",
            log_path: &format!("{logs_dir}/original"),
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();
    execution::mark_running(&pool, &original.id).await.unwrap();
    execution::mark_completed(&pool, &original.id, ExecutionStatus::Failed, Some(1))
        .await
        .unwrap();

    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{url}/executions/{}/replay", original.id))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let new_id = body["execution_id"].as_str().expect("execution_id");

    // Wait for the spawned executor to complete
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let exec = execution::get_by_id(&pool, new_id).await.unwrap();
        if exec.status.is_terminal() {
            assert_eq!(exec.status, ExecutionStatus::Success);
            assert_eq!(exec.exit_code, Some(0));
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("replay execution did not complete within 5 seconds");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // Verify stdout log was written
    let stdout_path = std::path::Path::new(logs_dir).join(new_id).join("stdout.log");
    let stdout = tokio::fs::read_to_string(&stdout_path).await.unwrap();
    assert_eq!(stdout.trim(), "replayed-output");
}

// --- Execution detail page tests ---

#[tokio::test]
async fn execution_detail_returns_404_for_unknown_id() {
    let (url, token) = spawn_authed_server(AppConfig::default()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/executions/nonexistent"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn execution_detail_renders_metadata() {
    use sendword::models::execution::{self, NewExecution};
    use sendword::models::ExecutionStatus;

    let config = AppConfig {
        hooks: vec![make_test_hook("Test Hook", "test-hook", "echo hello")],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;

    let exec = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "data/logs/test",
            trigger_source: "10.0.0.5",
            request_payload: r#"{"key": "value"}"#,
            retry_of: None,
        },
    )
    .await
    .unwrap();
    execution::mark_running(state.db.pool(), &exec.id)
        .await
        .unwrap();
    execution::mark_completed(state.db.pool(), &exec.id, ExecutionStatus::Success, Some(0))
        .await
        .unwrap();

    let exec_id = exec.id.clone();
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/executions/{exec_id}"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // Execution ID
    assert!(body.contains(&exec_id), "should show full execution ID");
    assert!(
        body.contains(&exec_id[..8]),
        "should show truncated execution ID in title"
    );

    // Hook link
    assert!(
        body.contains("/hooks/test-hook"),
        "should link to hook detail"
    );
    assert!(body.contains("test-hook"), "should show hook slug");

    // Status
    assert!(body.contains("success"), "should show status");

    // Exit code
    assert!(body.contains(">0<"), "should show exit code 0");

    // Source IP
    assert!(body.contains("10.0.0.5"), "should show trigger source");

    // Timing labels
    assert!(body.contains("Triggered at"), "should show triggered at label");
    assert!(body.contains("Started at"), "should show started at label");
    assert!(body.contains("Completed at"), "should show completed at label");
    assert!(body.contains("Duration"), "should show duration label");

    // Replay button
    assert!(body.contains("Replay"), "should have replay button");
    assert!(
        body.contains(&format!("/executions/{exec_id}/replay")),
        "replay should target correct URL"
    );
}

#[tokio::test]
async fn execution_detail_shows_failed_status_with_red_badge() {
    use sendword::models::execution::{self, NewExecution};
    use sendword::models::ExecutionStatus;

    let config = AppConfig {
        hooks: vec![make_test_hook("Test Hook", "test-hook", "exit 1")],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;

    let exec = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "data/logs/test",
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();
    execution::mark_running(state.db.pool(), &exec.id)
        .await
        .unwrap();
    execution::mark_completed(state.db.pool(), &exec.id, ExecutionStatus::Failed, Some(1))
        .await
        .unwrap();

    let exec_id = exec.id.clone();
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/executions/{exec_id}"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("failed"), "should show failed status");
    assert!(body.contains("bg-red-100"), "should use red badge for failed");
}

#[tokio::test]
async fn execution_detail_reads_log_files() {
    use sendword::models::execution::{self, NewExecution};
    use sendword::models::ExecutionStatus;

    let tmp = tempfile::TempDir::new().unwrap();
    let logs_dir = tmp.path().to_str().unwrap();

    let config = AppConfig {
        logs: sendword::config::LogsConfig {
            dir: logs_dir.into(),
        },
        hooks: vec![make_test_hook("Test Hook", "test-hook", "echo hello")],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;

    let exec_id = sendword::id::new_id();
    let log_path = format!("{logs_dir}/{exec_id}");

    // Create log directory and files before creating the execution
    tokio::fs::create_dir_all(&log_path).await.unwrap();
    tokio::fs::write(format!("{log_path}/stdout.log"), "hello from stdout")
        .await
        .unwrap();
    tokio::fs::write(format!("{log_path}/stderr.log"), "warning from stderr")
        .await
        .unwrap();

    let exec = execution::create(
        state.db.pool(),
        &NewExecution {
            id: Some(&exec_id),
            hook_slug: "test-hook",
            log_path: &log_path,
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();
    execution::mark_running(state.db.pool(), &exec.id)
        .await
        .unwrap();
    execution::mark_completed(state.db.pool(), &exec.id, ExecutionStatus::Success, Some(0))
        .await
        .unwrap();

    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/executions/{exec_id}"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(
        body.contains("hello from stdout"),
        "should display stdout log content"
    );
    assert!(
        body.contains("warning from stderr"),
        "should display stderr log content"
    );
}

#[tokio::test]
async fn execution_detail_shows_fallback_when_logs_missing() {
    use sendword::models::execution::{self, NewExecution};

    let config = AppConfig {
        logs: sendword::config::LogsConfig {
            dir: "/tmp/nonexistent-sendword-logs-detail".into(),
        },
        hooks: vec![make_test_hook("Test Hook", "test-hook", "echo hello")],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;

    let exec = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "/tmp/nonexistent-sendword-logs-detail/test",
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();

    let exec_id = exec.id.clone();
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/executions/{exec_id}"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    // The fallback message should appear (at least twice, once for stdout and once for stderr)
    let count = body.matches("No output captured.").count();
    assert!(
        count >= 2,
        "should show 'No output captured.' for both stdout and stderr, found {count} occurrences"
    );
}

#[tokio::test]
async fn execution_detail_shows_retry_info_when_replay() {
    use sendword::models::execution::{self, NewExecution};
    use sendword::models::ExecutionStatus;

    let config = AppConfig {
        hooks: vec![make_test_hook("Test Hook", "test-hook", "echo hello")],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;

    // Create original execution
    let original = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "data/logs/original",
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();
    execution::mark_running(state.db.pool(), &original.id)
        .await
        .unwrap();
    execution::mark_completed(
        state.db.pool(),
        &original.id,
        ExecutionStatus::Failed,
        Some(1),
    )
    .await
    .unwrap();

    // Create a replay linked to the original
    let replay = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "data/logs/replay",
            trigger_source: "replay",
            request_payload: "{}",
            retry_of: Some(&original.id),
        },
    )
    .await
    .unwrap();

    let replay_id = replay.id.clone();
    let original_id = original.id.clone();
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/executions/{replay_id}"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("Retry Info"), "should show retry info section");
    assert!(body.contains("Replay of"), "should show 'Replay of' label");
    assert!(
        body.contains(&format!("/executions/{original_id}")),
        "should link to original execution"
    );
    assert!(
        body.contains(&original_id[..8]),
        "should show truncated original ID"
    );
}

#[tokio::test]
async fn execution_detail_hides_retry_section_when_not_applicable() {
    use sendword::models::execution::{self, NewExecution};

    let config = AppConfig {
        hooks: vec![make_test_hook("Test Hook", "test-hook", "echo hello")],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;

    let exec = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "data/logs/test",
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();

    let exec_id = exec.id.clone();
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/executions/{exec_id}"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(
        !body.contains("Retry Info"),
        "should not show retry info section when retry_count is 0 and retry_of is None"
    );
}

#[tokio::test]
async fn execution_detail_shows_pending_status_with_yellow_badge() {
    use sendword::models::execution::{self, NewExecution};

    let config = AppConfig {
        hooks: vec![make_test_hook("Test Hook", "test-hook", "echo hello")],
        ..AppConfig::default()
    };

    let state = test_state(config).await;
    let token = create_test_session(&state).await;

    // Execution stays in pending status (not started)
    let exec = execution::create(
        state.db.pool(),
        &NewExecution {
            id: None,
            hook_slug: "test-hook",
            log_path: "data/logs/test",
            trigger_source: "127.0.0.1",
            request_payload: "{}",
            retry_of: None,
        },
    )
    .await
    .unwrap();

    let exec_id = exec.id.clone();
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/executions/{exec_id}"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("pending"), "should show pending status");
    assert!(
        body.contains("bg-yellow-100"),
        "should use yellow badge for pending"
    );
    // Started at should show dash when not yet started
    assert!(
        body.contains("Started at"),
        "should show started at label"
    );
}

// --- Auth redirect tests ---

#[tokio::test]
async fn unauthenticated_dashboard_redirects_to_login() {
    let url = spawn_test_server().await;
    let client = client_no_redirect();
    let resp = client.get(format!("{url}/")).send().await.unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn unauthenticated_hook_detail_redirects_to_login() {
    let url = spawn_test_server().await;
    let client = client_no_redirect();
    let resp = client
        .get(format!("{url}/hooks/some-slug"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn unauthenticated_execution_detail_redirects_to_login() {
    let url = spawn_test_server().await;
    let client = client_no_redirect();
    let resp = client
        .get(format!("{url}/executions/some-id"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn unauthenticated_replay_redirects_to_login() {
    let url = spawn_test_server().await;
    let client = client_no_redirect();
    let resp = client
        .post(format!("{url}/executions/some-id/replay"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn webhook_trigger_still_works_without_session_cookie() {
    let url = spawn_test_server().await;
    let client = reqwest::Client::new();
    // POST to a nonexistent hook should return 404, not a redirect
    let resp = client
        .post(format!("{url}/hook/nonexistent"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// --- Script editor tests ---

#[tokio::test]
async fn scripts_new_requires_auth() {
    let url = spawn_test_server().await;
    let client = client_no_redirect();
    let resp = client
        .get(format!("{url}/scripts/new"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/login");
}

#[tokio::test]
async fn scripts_new_renders_editor() {
    let (url, token) = spawn_authed_server(AppConfig::default()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/scripts/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("New script"));
    assert!(body.contains("textarea"));
    assert!(body.contains("Create"));
}

#[tokio::test]
async fn scripts_create_and_edit_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let scripts_dir = tmp.path().join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();

    let config = AppConfig {
        scripts: sendword::config::ScriptsConfig {
            dir: scripts_dir.to_str().unwrap().into(),
        },
        ..AppConfig::default()
    };

    let (url, token) = spawn_authed_server(config).await;
    let client = client_no_redirect();

    // Create a new script
    let resp = client
        .post(format!("{url}/scripts/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("filename=deploy.sh&content=%23!%2Fbin%2Fbash%0Aecho+hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.contains("/scripts/deploy.sh"),
        "should redirect to script editor: {location}"
    );

    // Verify file was written
    let file_path = scripts_dir.join("deploy.sh");
    assert!(file_path.exists(), "script file should exist");
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("echo hello"));

    // Verify executable bit
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file_path).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "file should be executable, mode: {mode:o}");
    }

    // Edit the script (GET)
    let client_follow = reqwest::Client::new();
    let resp = client_follow
        .get(format!("{url}/scripts/deploy.sh"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("deploy.sh"));
    assert!(body.contains("echo hello"));
    assert!(body.contains("Save"));
    assert!(body.contains("Delete"));

    // Save the script (POST)
    let resp = client
        .post(format!("{url}/scripts/deploy.sh"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("content=%23!%2Fbin%2Fbash%0Aecho+updated")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);

    let content = std::fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("echo updated"));
}

#[tokio::test]
async fn scripts_create_rejects_invalid_filename() {
    let tmp = tempfile::tempdir().unwrap();
    let scripts_dir = tmp.path().join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();

    let config = AppConfig {
        scripts: sendword::config::ScriptsConfig {
            dir: scripts_dir.to_str().unwrap().into(),
        },
        ..AppConfig::default()
    };

    let (url, token) = spawn_authed_server(config).await;
    let client = client_no_redirect();

    // Leading dot
    let resp = client
        .post(format!("{url}/scripts/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("filename=.hidden&content=bad")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("error="), "should redirect with error: {location}");

    // Path traversal
    let resp = client
        .post(format!("{url}/scripts/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("filename=..%2Fetc%2Fpasswd&content=bad")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("error="), "should redirect with error: {location}");
}

#[tokio::test]
async fn scripts_create_rejects_duplicate() {
    let tmp = tempfile::tempdir().unwrap();
    let scripts_dir = tmp.path().join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();

    // Pre-create a file
    std::fs::write(scripts_dir.join("existing.sh"), "#!/bin/bash").unwrap();

    let config = AppConfig {
        scripts: sendword::config::ScriptsConfig {
            dir: scripts_dir.to_str().unwrap().into(),
        },
        ..AppConfig::default()
    };

    let (url, token) = spawn_authed_server(config).await;
    let client = client_no_redirect();

    let resp = client
        .post(format!("{url}/scripts/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("filename=existing.sh&content=overwrite+attempt")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("error="), "should redirect with error: {location}");

    // Original content should be untouched
    let content = std::fs::read_to_string(scripts_dir.join("existing.sh")).unwrap();
    assert_eq!(content, "#!/bin/bash");
}

#[tokio::test]
async fn scripts_delete_removes_file() {
    let tmp = tempfile::tempdir().unwrap();
    let scripts_dir = tmp.path().join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();

    let file_path = scripts_dir.join("doomed.sh");
    std::fs::write(&file_path, "#!/bin/bash\necho goodbye").unwrap();

    let config = AppConfig {
        scripts: sendword::config::ScriptsConfig {
            dir: scripts_dir.to_str().unwrap().into(),
        },
        ..AppConfig::default()
    };

    let (url, token) = spawn_authed_server(config).await;
    let client = client_no_redirect();

    let resp = client
        .post(format!("{url}/scripts/doomed.sh/delete"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("/scripts"), "should redirect to scripts list: {location}");
    assert!(location.contains("success="), "should have success flash: {location}");

    assert!(!file_path.exists(), "file should be deleted");
}

#[tokio::test]
async fn scripts_edit_returns_404_for_nonexistent() {
    let tmp = tempfile::tempdir().unwrap();
    let scripts_dir = tmp.path().join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();

    let config = AppConfig {
        scripts: sendword::config::ScriptsConfig {
            dir: scripts_dir.to_str().unwrap().into(),
        },
        ..AppConfig::default()
    };

    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/scripts/nonexistent.sh"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// --- Webhook auth integration tests ---

fn config_with_hook(hook: HookConfig) -> AppConfig {
    AppConfig {
        hooks: vec![hook],
        ..AppConfig::default()
    }
}

fn shell_hook(slug: &str) -> HookConfig {
    HookConfig {
        name: slug.to_owned(),
        slug: slug.to_owned(),
        description: String::new(),
        enabled: true,
        auth: None,
        executor: ExecutorConfig::Shell {
            command: "echo ok".to_owned(),
        },
        env: Default::default(),
        cwd: None,
        timeout: None,
        retries: None,
        rate_limit: None,
        payload: None,
        trigger_rules: None,
    }
}

#[tokio::test]
async fn trigger_public_hook_succeeds_without_auth() {
    let hook = shell_hook("public");
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/public"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn trigger_bearer_hook_with_valid_token_succeeds() {
    let mut hook = shell_hook("bearer-test");
    hook.auth = Some(HookAuthConfig::Bearer {
        token: "test-token-123".to_owned(),
    });
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/bearer-test"))
        .header("Authorization", "Bearer test-token-123")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn trigger_bearer_hook_without_token_returns_401() {
    let mut hook = shell_hook("bearer-noauth");
    hook.auth = Some(HookAuthConfig::Bearer {
        token: "secret".to_owned(),
    });
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/bearer-noauth"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn trigger_bearer_hook_with_wrong_token_returns_401() {
    let mut hook = shell_hook("bearer-wrong");
    hook.auth = Some(HookAuthConfig::Bearer {
        token: "correct-token".to_owned(),
    });
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/bearer-wrong"))
        .header("Authorization", "Bearer wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn trigger_bearer_hook_with_env_var_token_succeeds() {
    // SAFETY: test-only, setting env for auth verification
    unsafe { std::env::set_var("TEST_BEARER_TOKEN_INTEG", "env-token-value") };
    let mut hook = shell_hook("bearer-env");
    hook.auth = Some(HookAuthConfig::Bearer {
        token: "${TEST_BEARER_TOKEN_INTEG}".to_owned(),
    });
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/bearer-env"))
        .header("Authorization", "Bearer env-token-value")
        .send()
        .await
        .unwrap();
    // SAFETY: test-only cleanup
    unsafe { std::env::remove_var("TEST_BEARER_TOKEN_INTEG") };
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn trigger_hmac_hook_with_valid_signature_succeeds() {
    use ring::hmac;

    let secret = "test-hmac-secret";
    let body = b"test-body-content";

    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, body);
    let hex_sig: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();

    let mut hook = shell_hook("hmac-test");
    hook.auth = Some(HookAuthConfig::Hmac {
        header: "X-Hub-Signature-256".to_owned(),
        algorithm: HmacAlgorithm::Sha256,
        secret: secret.to_owned(),
    });
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/hmac-test"))
        .header("X-Hub-Signature-256", format!("sha256={hex_sig}"))
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn trigger_hmac_hook_with_wrong_signature_returns_401() {
    let mut hook = shell_hook("hmac-wrong");
    hook.auth = Some(HookAuthConfig::Hmac {
        header: "X-Hub-Signature-256".to_owned(),
        algorithm: HmacAlgorithm::Sha256,
        secret: "the-real-secret".to_owned(),
    });
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/hmac-wrong"))
        .header(
            "X-Hub-Signature-256",
            "sha256=0000000000000000000000000000000000000000000000000000000000000000",
        )
        .body("some body")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn trigger_hmac_hook_without_signature_header_returns_401() {
    let mut hook = shell_hook("hmac-nosig");
    hook.auth = Some(HookAuthConfig::Hmac {
        header: "X-Hub-Signature-256".to_owned(),
        algorithm: HmacAlgorithm::Sha256,
        secret: "secret".to_owned(),
    });
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/hmac-nosig"))
        .body("some body")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

// --- Auth config round-trip via web UI ---

#[tokio::test]
async fn create_hook_with_bearer_auth_via_form() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let config_path = dir.path().join("sendword.toml");
    std::fs::write(&config_path, "[server]\nport = 8080\n").expect("write");

    let config = AppConfig::load_from(
        config_path.to_str().unwrap(),
        "nonexistent.json",
    ).expect("valid config");

    let db = Db::new_in_memory().await.expect("db");
    db.migrate().await.expect("migrate");
    let templates = Templates::new(Templates::default_dir());
    let state = AppState::new(config, &config_path, db, templates);
    let token = create_test_session(&state).await;
    let url = spawn_server(state).await;

    let client = client_no_redirect();
    let resp = client
        .post(format!("{url}/hooks/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=Bearer+Hook&slug=bearer-hook&command=echo+ok&enabled=true\
             &auth_mode=bearer&auth_token=%24%7BWEBHOOK_TOKEN%7D\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay="
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "expected redirect after create");

    // Verify the hook detail page shows bearer auth
    let detail = reqwest::Client::new()
        .get(format!("{url}/hooks/bearer-hook"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(detail.status(), 200);
    let body = detail.text().await.unwrap();
    assert!(body.contains("bearer"), "detail page should show bearer auth mode");
}

#[tokio::test]
async fn create_hook_with_hmac_auth_via_form() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let config_path = dir.path().join("sendword.toml");
    std::fs::write(&config_path, "[server]\nport = 8080\n").expect("write");

    let config = AppConfig::load_from(
        config_path.to_str().unwrap(),
        "nonexistent.json",
    ).expect("valid config");

    let db = Db::new_in_memory().await.expect("db");
    db.migrate().await.expect("migrate");
    let templates = Templates::new(Templates::default_dir());
    let state = AppState::new(config, &config_path, db, templates);
    let token = create_test_session(&state).await;
    let url = spawn_server(state).await;

    let client = client_no_redirect();
    let resp = client
        .post(format!("{url}/hooks/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=HMAC+Hook&slug=hmac-hook&command=echo+ok&enabled=true\
             &auth_mode=hmac&auth_header=X-Hub-Signature-256\
             &auth_algorithm=sha256&auth_secret=%24%7BWEBHOOK_SECRET%7D\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay="
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "expected redirect after create");

    // Verify the TOML file contains the auth config
    let toml_content = std::fs::read_to_string(&config_path).unwrap();
    assert!(toml_content.contains("mode = \"hmac\""), "TOML should contain hmac mode");
    assert!(toml_content.contains("X-Hub-Signature-256"), "TOML should contain header name");
}

#[tokio::test]
async fn edit_hook_to_add_bearer_auth() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let config_path = dir.path().join("sendword.toml");
    let initial_toml = r#"
[server]
port = 8080

[[hooks]]
name = "Public Hook"
slug = "public-hook"
[hooks.executor]
type = "shell"
command = "echo ok"
"#;
    std::fs::write(&config_path, initial_toml).expect("write");

    let config = AppConfig::load_from(
        config_path.to_str().unwrap(),
        "nonexistent.json",
    ).expect("valid config");

    let db = Db::new_in_memory().await.expect("db");
    db.migrate().await.expect("migrate");
    let templates = Templates::new(Templates::default_dir());
    let state = AppState::new(config, &config_path, db, templates);
    let token = create_test_session(&state).await;
    let url = spawn_server(state).await;

    let client = client_no_redirect();
    let resp = client
        .post(format!("{url}/hooks/public-hook/edit"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=Public+Hook&slug=public-hook&command=echo+ok&enabled=true\
             &auth_mode=bearer&auth_token=my-secret\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay="
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "expected redirect after edit");

    // Verify TOML now has auth section
    let toml_content = std::fs::read_to_string(&config_path).unwrap();
    assert!(toml_content.contains("mode = \"bearer\""), "TOML should contain bearer mode");
    assert!(toml_content.contains("my-secret"), "TOML should contain token");
}

#[tokio::test]
async fn trigger_explicit_none_auth_hook_succeeds() {
    let mut hook = shell_hook("explicit-none");
    hook.auth = Some(HookAuthConfig::None);
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/explicit-none"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn trigger_hmac_hook_raw_hex_without_prefix_succeeds() {
    use ring::hmac;

    let secret = "raw-hex-secret";
    let body = b"raw-hex-body";

    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, body);
    let hex_sig: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();

    let mut hook = shell_hook("hmac-raw");
    hook.auth = Some(HookAuthConfig::Hmac {
        header: "X-Signature".to_owned(),
        algorithm: HmacAlgorithm::Sha256,
        secret: secret.to_owned(),
    });
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    // Send raw hex without the "sha256=" prefix
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/hmac-raw"))
        .header("X-Signature", &hex_sig)
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn trigger_hmac_hook_with_env_var_secret_succeeds() {
    use ring::hmac;

    let secret = "env-hmac-secret-value";
    // SAFETY: test-only, setting env for auth verification
    unsafe { std::env::set_var("TEST_HMAC_SECRET_INTEG", secret) };

    let body = b"hmac-env-body";
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, body);
    let hex_sig: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();

    let mut hook = shell_hook("hmac-env");
    hook.auth = Some(HookAuthConfig::Hmac {
        header: "X-Hub-Signature-256".to_owned(),
        algorithm: HmacAlgorithm::Sha256,
        secret: "${TEST_HMAC_SECRET_INTEG}".to_owned(),
    });
    let url = {
        let state = test_state(config_with_hook(hook)).await;
        spawn_server(state).await
    };
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/hmac-env"))
        .header("X-Hub-Signature-256", format!("sha256={hex_sig}"))
        .body(body.to_vec())
        .send()
        .await
        .unwrap();
    // SAFETY: test-only cleanup
    unsafe { std::env::remove_var("TEST_HMAC_SECRET_INTEG") };
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn trigger_bearer_hook_returns_execution_id() {
    let mut hook = shell_hook("bearer-exec");
    hook.auth = Some(HookAuthConfig::Bearer {
        token: "exec-token".to_owned(),
    });
    let state = test_state(config_with_hook(hook)).await;
    let pool = state.db.pool().clone();
    let url = spawn_server(state).await;

    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/bearer-exec"))
        .header("Authorization", "Bearer exec-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let exec_id = body["execution_id"]
        .as_str()
        .expect("response should contain execution_id");
    assert!(!exec_id.is_empty(), "execution_id should be non-empty");

    // Verify the execution record exists in the database
    let exec = sendword::models::execution::get_by_id(&pool, exec_id)
        .await
        .unwrap();
    assert_eq!(exec.hook_slug, "bearer-exec");
}

// --- Trigger attempt pipeline tests ---

#[tokio::test]
async fn trigger_creates_fired_attempt_with_execution_id() {
    let hook = shell_hook("fire-attempt");
    let state = test_state(config_with_hook(hook)).await;
    let pool = state.db.pool().clone();
    let url = spawn_server(state).await;

    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/fire-attempt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let exec_id = body["execution_id"].as_str().expect("execution_id in response");

    let attempts = trigger_attempt::list_by_hook(&pool, "fire-attempt", 10, 0)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);

    let attempt = &attempts[0];
    assert_eq!(attempt.hook_slug, "fire-attempt");
    assert_eq!(attempt.status, TriggerAttemptStatus::Fired);
    assert_eq!(attempt.execution_id.as_deref(), Some(exec_id));
    assert!(!attempt.source_ip.is_empty(), "source_ip should be populated");
}

#[tokio::test]
async fn trigger_auth_failure_creates_auth_failed_attempt() {
    let mut hook = shell_hook("auth-fail-attempt");
    hook.auth = Some(HookAuthConfig::Bearer {
        token: "correct-token".to_owned(),
    });
    let state = test_state(config_with_hook(hook)).await;
    let pool = state.db.pool().clone();
    let url = spawn_server(state).await;

    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/auth-fail-attempt"))
        .header("Authorization", "Bearer wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    let attempts = trigger_attempt::list_by_hook(&pool, "auth-fail-attempt", 10, 0)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);

    let attempt = &attempts[0];
    assert_eq!(attempt.hook_slug, "auth-fail-attempt");
    assert_eq!(attempt.status, TriggerAttemptStatus::AuthFailed);
    assert!(attempt.execution_id.is_none(), "auth_failed should have no execution_id");
    assert!(!attempt.source_ip.is_empty(), "source_ip should be populated");
    assert!(!attempt.reason.is_empty(), "reason should describe denial");
}

#[tokio::test]
async fn trigger_auth_failure_no_header_creates_auth_failed_attempt() {
    let mut hook = shell_hook("auth-noheader");
    hook.auth = Some(HookAuthConfig::Bearer {
        token: "secret".to_owned(),
    });
    let state = test_state(config_with_hook(hook)).await;
    let pool = state.db.pool().clone();
    let url = spawn_server(state).await;

    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/auth-noheader"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    let attempts = trigger_attempt::list_by_hook(&pool, "auth-noheader", 10, 0)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].status, TriggerAttemptStatus::AuthFailed);
    assert!(attempts[0].execution_id.is_none());
}

#[tokio::test]
async fn trigger_invalid_json_creates_validation_failed_attempt() {
    let mut hook = shell_hook("bad-json");
    hook.payload = Some(PayloadSchema {
        fields: vec![PayloadField {
            name: "repo".to_owned(),
            field_type: FieldType::String,
            required: true,
        }],
    });
    let state = test_state(config_with_hook(hook)).await;
    let pool = state.db.pool().clone();
    let url = spawn_server(state).await;

    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/bad-json"))
        .header("Content-Type", "application/json")
        .body("not valid json {{{")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let attempts = trigger_attempt::list_by_hook(&pool, "bad-json", 10, 0)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);

    let attempt = &attempts[0];
    assert_eq!(attempt.status, TriggerAttemptStatus::ValidationFailed);
    assert!(attempt.execution_id.is_none());
    assert!(attempt.reason.contains("invalid JSON"), "reason should mention invalid JSON");
}

#[tokio::test]
async fn trigger_schema_validation_failure_creates_validation_failed_attempt() {
    let mut hook = shell_hook("schema-fail");
    hook.payload = Some(PayloadSchema {
        fields: vec![PayloadField {
            name: "version".to_owned(),
            field_type: FieldType::Number,
            required: true,
        }],
    });
    let state = test_state(config_with_hook(hook)).await;
    let pool = state.db.pool().clone();
    let url = spawn_server(state).await;

    // Send valid JSON, but missing the required "version" field
    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/schema-fail"))
        .header("Content-Type", "application/json")
        .body(r#"{"other": "value"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);

    let attempts = trigger_attempt::list_by_hook(&pool, "schema-fail", 10, 0)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);

    let attempt = &attempts[0];
    assert_eq!(attempt.status, TriggerAttemptStatus::ValidationFailed);
    assert!(attempt.execution_id.is_none());
    assert!(
        attempt.reason.contains("validation failed"),
        "reason should describe validation failure"
    );
}

#[tokio::test]
async fn trigger_with_x_forwarded_for_records_forwarded_ip() {
    let hook = shell_hook("forwarded-ip");
    let state = test_state(config_with_hook(hook)).await;
    let pool = state.db.pool().clone();
    let url = spawn_server(state).await;

    let resp = reqwest::Client::new()
        .post(format!("{url}/hook/forwarded-ip"))
        .header("X-Forwarded-For", "203.0.113.50, 10.0.0.1")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let attempts = trigger_attempt::list_by_hook(&pool, "forwarded-ip", 10, 0)
        .await
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].source_ip, "203.0.113.50");
}

// --- Trigger attempt list (Web UI) tests ---

#[tokio::test]
async fn attempt_list_returns_404_for_unknown_hook() {
    let (url, token) = spawn_authed_server(AppConfig::default()).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{url}/hooks/nonexistent/attempts"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn attempt_list_empty_shows_no_attempts_message() {
    let config = AppConfig {
        hooks: vec![make_test_hook("Fresh", "fresh-hook", "echo hi")],
        ..AppConfig::default()
    };
    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/hooks/fresh-hook/attempts"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("No trigger attempts"),
        "empty list should show 'No trigger attempts' message"
    );
}

#[tokio::test]
async fn attempt_list_shows_fired_after_trigger() {
    let hook = shell_hook("list-fired");
    let state = test_state(config_with_hook(hook)).await;
    let token = create_test_session(&state).await;
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    // Trigger the hook to create a fired attempt
    let resp = client
        .post(format!("{url}/hook/list-fired"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Fetch the attempts partial
    let resp = client
        .get(format!("{url}/hooks/list-fired/attempts"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("fired"), "should contain fired status");
    assert!(body.contains("bg-green-100"), "fired badge should use green color");
    assert!(body.contains("<table"), "should render as HTML table");
}

#[tokio::test]
async fn attempt_list_status_filter_shows_only_matching() {
    let mut hook = shell_hook("filter-test");
    hook.auth = Some(HookAuthConfig::Bearer {
        token: "secret".to_owned(),
    });
    let state = test_state(config_with_hook(hook)).await;
    let token = create_test_session(&state).await;
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    // Create a fired attempt (with correct auth)
    let resp = client
        .post(format!("{url}/hook/filter-test"))
        .header("Authorization", "Bearer secret")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Create an auth_failed attempt (wrong token)
    let resp = client
        .post(format!("{url}/hook/filter-test"))
        .header("Authorization", "Bearer wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // Filter by auth_failed: should show auth_failed but not fired
    let resp = client
        .get(format!("{url}/hooks/filter-test/attempts?status=auth_failed"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("auth_failed"), "should show auth_failed attempt");
    assert!(body.contains("bg-red-100"), "auth_failed badge should use red color");
    // The fired attempt should not be in the filtered results
    assert!(
        !body.contains("bg-green-100"),
        "filtered list should not contain fired (green) badge"
    );

    // Filter by fired: should show only fired
    let resp = client
        .get(format!("{url}/hooks/filter-test/attempts?status=fired"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("fired"), "should show fired attempt");
    assert!(
        !body.contains("auth_failed"),
        "filtered list should not contain auth_failed"
    );

    // All (no filter): should show both
    let resp = client
        .get(format!("{url}/hooks/filter-test/attempts"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("fired"), "unfiltered should show fired");
    assert!(body.contains("auth_failed"), "unfiltered should show auth_failed");
}

#[tokio::test]
async fn attempt_list_unknown_status_filter_returns_all() {
    let hook = shell_hook("unknown-filter");
    let state = test_state(config_with_hook(hook)).await;
    let token = create_test_session(&state).await;
    let url = spawn_server(state).await;
    let client = reqwest::Client::new();

    // Create a fired attempt
    client
        .post(format!("{url}/hook/unknown-filter"))
        .send()
        .await
        .unwrap();

    // Unknown status filter falls through to unfiltered (parse returns None)
    let resp = client
        .get(format!("{url}/hooks/unknown-filter/attempts?status=bogus"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("fired"), "unknown filter should fall through to showing all");
}

#[tokio::test]
async fn hook_detail_contains_trigger_attempts_section() {
    let config = AppConfig {
        hooks: vec![make_test_hook("Attempt Hook", "attempt-hook", "echo hi")],
        ..AppConfig::default()
    };
    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/hooks/attempt-hook"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(
        body.contains("Trigger Attempts"),
        "hook detail should contain Trigger Attempts heading"
    );
    assert!(
        body.contains("attempt-list"),
        "hook detail should contain attempt-list container"
    );
    assert!(
        body.contains("attempt-filters"),
        "hook detail should contain filter buttons"
    );
    assert!(
        body.contains("/hooks/attempt-hook/attempts"),
        "hook detail should contain HTMX URL for attempts"
    );
    assert!(
        body.contains("status=fired"),
        "hook detail should have fired filter button"
    );
    assert!(
        body.contains("status=auth_failed"),
        "hook detail should have auth_failed filter button"
    );
    assert!(
        body.contains("status=validation_failed"),
        "hook detail should have validation_failed filter button"
    );
}

#[tokio::test]
async fn attempt_list_requires_auth() {
    let config = AppConfig {
        hooks: vec![shell_hook("auth-required")],
        ..AppConfig::default()
    };
    let state = test_state(config).await;
    // Enable auth by creating a user (but don't use the session)
    create_test_session(&state).await;
    let url = spawn_server(state).await;

    let client = client_no_redirect();
    let resp = client
        .get(format!("{url}/hooks/auth-required/attempts"))
        .send()
        .await
        .unwrap();
    // Should redirect to login (303) when no session cookie
    assert_eq!(resp.status(), 303);
}

// ---------------------------------------------------------------------------
// Trigger rules UI tests
// ---------------------------------------------------------------------------

/// Set up a test server backed by an on-disk TOML config file so that
/// config_writer round-trips can be verified.
async fn spawn_file_backed_server(initial_toml: &str) -> (String, String, std::path::PathBuf, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let config_path = dir.path().join("sendword.toml");
    std::fs::write(&config_path, initial_toml).expect("write toml");

    let config = AppConfig::load_from(
        config_path.to_str().unwrap(),
        "nonexistent.json",
    ).expect("valid config");

    let db = Db::new_in_memory().await.expect("db");
    db.migrate().await.expect("migrate");
    let templates = Templates::new(Templates::default_dir());
    let state = AppState::new(config, &config_path, db, templates);
    let token = create_test_session(&state).await;
    let url = spawn_server(state).await;
    (url, token, config_path, dir)
}

#[tokio::test]
async fn create_hook_with_payload_filters_via_form() {
    let (url, token, config_path, _dir) = spawn_file_backed_server("[server]\nport = 8080\n").await;
    let client = client_no_redirect();

    let resp = client
        .post(format!("{url}/hooks/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=Filter+Hook&slug=filter-hook&command=echo+ok&enabled=true\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay=\
             &auth_mode=none\
             &trigger_filters_text=action%3Aequals%3Adeploy%0Aenv%3Acontains%3Aprod\
             &trigger_windows_text=&trigger_cooldown=&trigger_rate_max=&trigger_rate_window="
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "expected redirect after create");

    let toml_content = std::fs::read_to_string(&config_path).unwrap();
    assert!(toml_content.contains("payload_filters"), "TOML should contain payload_filters");
    assert!(toml_content.contains("\"action\""), "TOML should contain field 'action'");
    assert!(toml_content.contains("\"equals\""), "TOML should contain operator 'equals'");
    assert!(toml_content.contains("\"deploy\""), "TOML should contain value 'deploy'");
    assert!(toml_content.contains("\"env\""), "TOML should contain field 'env'");
    assert!(toml_content.contains("\"contains\""), "TOML should contain operator 'contains'");
}

#[tokio::test]
async fn hook_detail_displays_trigger_rules() {
    use sendword::config::{
        FilterOperator, PayloadFilter, TimeWindow, TriggerRateLimit, TriggerRules,
    };
    use std::time::Duration;

    let mut hook = shell_hook("ruled-hook");
    hook.trigger_rules = Some(TriggerRules {
        payload_filters: Some(vec![PayloadFilter {
            field: "action".into(),
            operator: FilterOperator::Equals,
            value: Some("deploy".into()),
        }]),
        time_windows: Some(vec![TimeWindow {
            days: vec!["Mon".into(), "Fri".into()],
            start_time: "09:00".into(),
            end_time: "17:00".into(),
        }]),
        cooldown: Some(Duration::from_secs(300)),
        rate_limit: Some(TriggerRateLimit {
            max_requests: 10,
            window: Duration::from_secs(3600),
        }),
    });

    let config = AppConfig {
        hooks: vec![hook],
        ..AppConfig::default()
    };
    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/hooks/ruled-hook"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("Trigger Rules"), "detail page should show Trigger Rules section");
    assert!(body.contains("action"), "detail page should show filter field");
    assert!(body.contains("equals"), "detail page should show filter operator");
    assert!(body.contains("deploy"), "detail page should show filter value");
    assert!(body.contains("Mon"), "detail page should show time window days");
    assert!(body.contains("09:00"), "detail page should show window start time");
    assert!(body.contains("17:00"), "detail page should show window end time");
    assert!(body.contains("5m"), "detail page should show cooldown");
    assert!(body.contains("10"), "detail page should show rate limit max");
    assert!(body.contains("1h"), "detail page should show rate limit window");
}

#[tokio::test]
async fn hook_without_trigger_rules_shows_no_trigger_rules_section() {
    let config = AppConfig {
        hooks: vec![shell_hook("plain-hook")],
        ..AppConfig::default()
    };
    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/hooks/plain-hook"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(!body.contains("Trigger Rules"), "detail page should not show Trigger Rules section when none configured");
}

#[tokio::test]
async fn create_hook_with_time_windows_and_cooldown() {
    let (url, token, config_path, _dir) = spawn_file_backed_server("[server]\nport = 8080\n").await;
    let client = client_no_redirect();

    let resp = client
        .post(format!("{url}/hooks/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=Window+Hook&slug=window-hook&command=echo+ok&enabled=true\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay=\
             &auth_mode=none\
             &trigger_filters_text=\
             &trigger_windows_text=Mon%2CTue%3A09%3A00-17%3A00\
             &trigger_cooldown=5m\
             &trigger_rate_max=&trigger_rate_window="
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "expected redirect after create");

    let toml_content = std::fs::read_to_string(&config_path).unwrap();
    assert!(toml_content.contains("time_windows"), "TOML should contain time_windows");
    assert!(toml_content.contains("09:00"), "TOML should contain start_time");
    assert!(toml_content.contains("17:00"), "TOML should contain end_time");
    assert!(toml_content.contains(r#"cooldown = "5m""#), "TOML should contain cooldown");
}

#[tokio::test]
async fn create_hook_with_rate_limit_via_form() {
    let (url, token, config_path, _dir) = spawn_file_backed_server("[server]\nport = 8080\n").await;
    let client = client_no_redirect();

    let resp = client
        .post(format!("{url}/hooks/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=Rate+Hook&slug=rate-hook&command=echo+ok&enabled=true\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay=\
             &auth_mode=none\
             &trigger_filters_text=&trigger_windows_text=&trigger_cooldown=\
             &trigger_rate_max=10&trigger_rate_window=1h"
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "expected redirect after create");

    let toml_content = std::fs::read_to_string(&config_path).unwrap();
    assert!(toml_content.contains("max_requests = 10"), "TOML should contain max_requests");
    assert!(toml_content.contains(r#"window = "1h""#), "TOML should contain window");
}

#[tokio::test]
async fn edit_hook_form_repopulates_trigger_rules() {
    use sendword::config::{FilterOperator, PayloadFilter, TriggerRules};

    let mut hook = shell_hook("editable-hook");
    hook.trigger_rules = Some(TriggerRules {
        payload_filters: Some(vec![PayloadFilter {
            field: "tag".into(),
            operator: FilterOperator::Contains,
            value: Some("release".into()),
        }]),
        time_windows: None,
        cooldown: None,
        rate_limit: None,
    });

    let config = AppConfig {
        hooks: vec![hook],
        ..AppConfig::default()
    };
    let (url, token) = spawn_authed_server(config).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{url}/hooks/editable-hook/edit"))
        .header("Cookie", format!("sendword_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("tag:contains:release"), "edit form should repopulate filter field");
}

#[tokio::test]
async fn edit_hook_to_add_trigger_filter() {
    let initial_toml = r#"
[server]
port = 8080

[[hooks]]
name = "Plain Hook"
slug = "plain-hook"
[hooks.executor]
type = "shell"
command = "echo ok"
"#;
    let (url, token, config_path, _dir) = spawn_file_backed_server(initial_toml).await;
    let client = client_no_redirect();

    let resp = client
        .post(format!("{url}/hooks/plain-hook/edit"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=Plain+Hook&slug=plain-hook&command=echo+ok&enabled=true\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay=\
             &auth_mode=none\
             &trigger_filters_text=status%3Aequals%3Asuccess\
             &trigger_windows_text=&trigger_cooldown=&trigger_rate_max=&trigger_rate_window="
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "expected redirect after edit");

    let toml_content = std::fs::read_to_string(&config_path).unwrap();
    assert!(toml_content.contains("\"status\""), "TOML should have status filter field");
    assert!(toml_content.contains("\"equals\""), "TOML should have equals operator");
    assert!(toml_content.contains("\"success\""), "TOML should have success value");
}

#[tokio::test]
async fn edit_hook_to_remove_trigger_rules() {
    let initial_toml = r#"
[server]
port = 8080

[[hooks]]
name = "Ruled Hook"
slug = "ruled-hook"
[hooks.executor]
type = "shell"
command = "echo ok"

[hooks.trigger_rules]
cooldown = "5m"
"#;
    let (url, token, config_path, _dir) = spawn_file_backed_server(initial_toml).await;
    let client = client_no_redirect();

    let resp = client
        .post(format!("{url}/hooks/ruled-hook/edit"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=Ruled+Hook&slug=ruled-hook&command=echo+ok&enabled=true\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay=\
             &auth_mode=none\
             &trigger_filters_text=&trigger_windows_text=&trigger_cooldown=\
             &trigger_rate_max=&trigger_rate_window="
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "expected redirect after edit");

    let toml_content = std::fs::read_to_string(&config_path).unwrap();
    assert!(!toml_content.contains("trigger_rules"), "TOML should not contain trigger_rules when cleared");
    assert!(!toml_content.contains("cooldown"), "TOML should not contain cooldown when cleared");
}

#[tokio::test]
async fn invalid_filter_line_returns_error() {
    let (url, token, _config_path, _dir) = spawn_file_backed_server("[server]\nport = 8080\n").await;
    let client = client_no_redirect();

    let resp = client
        .post(format!("{url}/hooks/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=Bad+Hook&slug=bad-hook&command=echo+ok&enabled=true\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay=\
             &auth_mode=none\
             &trigger_filters_text=nooperator\
             &trigger_windows_text=&trigger_cooldown=&trigger_rate_max=&trigger_rate_window="
        )
        .send()
        .await
        .unwrap();

    // Should redirect back with error query param
    assert_eq!(resp.status(), 303, "invalid filter should cause redirect with error");
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("error="), "redirect should contain error param");
}

#[tokio::test]
async fn exists_filter_requires_no_value() {
    let (url, token, config_path, _dir) = spawn_file_backed_server("[server]\nport = 8080\n").await;
    let client = client_no_redirect();

    let resp = client
        .post(format!("{url}/hooks/new"))
        .header("Cookie", format!("sendword_session={token}"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            "name=Exists+Hook&slug=exists-hook&command=echo+ok&enabled=true\
             &description=&cwd=&timeout=&env_text=\
             &retry_count=0&retry_backoff=exponential\
             &retry_initial_delay=&retry_max_delay=\
             &auth_mode=none\
             &trigger_filters_text=tag%3Aexists\
             &trigger_windows_text=&trigger_cooldown=&trigger_rate_max=&trigger_rate_window="
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "exists filter should succeed");

    let toml_content = std::fs::read_to_string(&config_path).unwrap();
    assert!(toml_content.contains("\"tag\""), "TOML should contain field 'tag'");
    assert!(toml_content.contains("\"exists\""), "TOML should contain operator 'exists'");
    // exists filter should not write a value key
    assert!(!toml_content.contains("value = "), "exists filter should not write value");
}
