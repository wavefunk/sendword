use std::sync::Arc;

use sendword::config::AppConfig;
use sendword::db::Db;
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
        axum::serve(listener, sendword::server::router(state))
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
                executor: ExecutorConfig::Shell {
                    command: "make deploy".into(),
                },
                env: HashMap::new(),
                cwd: None,
                timeout: None,
                retries: None,
                rate_limit: None,
            },
            HookConfig {
                name: "Run Tests".into(),
                slug: "run-tests".into(),
                description: String::new(),
                enabled: false,
                executor: ExecutorConfig::Shell {
                    command: "make test".into(),
                },
                env: HashMap::new(),
                cwd: None,
                timeout: None,
                retries: None,
                rate_limit: None,
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
            executor: ExecutorConfig::Shell {
                command: "echo ok".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
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
            executor: ExecutorConfig::Shell {
                command: "echo hi".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
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
        executor: ExecutorConfig::Shell {
            command: command.into(),
        },
        env: HashMap::from([("APP_ENV".into(), "test".into())]),
        cwd: Some("/tmp".into()),
        timeout: None,
        retries: None,
        rate_limit: None,
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
            executor: ExecutorConfig::Shell {
                command: "echo nope".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
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
            executor: ExecutorConfig::Shell {
                command: "echo replayed".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
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
            executor: ExecutorConfig::Shell {
                command: "echo replayed-output".into(),
            },
            env: HashMap::new(),
            cwd: None,
            timeout: None,
            retries: None,
            rate_limit: None,
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
