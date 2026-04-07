use std::sync::Arc;

use sendword::config::AppConfig;
use sendword::db::Db;
use sendword::server::AppState;
use sendword::templates::Templates;

async fn test_state(config: AppConfig) -> Arc<AppState> {
    let db = Db::new_in_memory().await.expect("in-memory db");
    db.migrate().await.expect("migration");
    let templates = Templates::new(Templates::default_dir());
    AppState::new(config, db, templates)
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

async fn spawn_test_server_with_config(config: AppConfig) -> String {
    let state = test_state(config).await;
    spawn_server(state).await
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
    let url = spawn_test_server().await;
    let resp = reqwest::get(format!("{url}/")).await.unwrap();
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
    let url = spawn_test_server().await;
    let client = reqwest::Client::new();

    // trigger_hook returns 404 for non-existent/disabled hooks
    let resp = client.post(format!("{url}/hook/test-hook")).send().await.unwrap();
    assert_eq!(resp.status(), 404);

    // hook_detail returns 404 for non-existent hooks
    let resp = client.get(format!("{url}/hooks/test-hook")).send().await.unwrap();
    assert_eq!(resp.status(), 404);

    let resp = client.get(format!("{url}/executions/some-id")).send().await.unwrap();
    assert_eq!(resp.status(), 501);

    let resp = client
        .post(format!("{url}/executions/some-id/replay"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 501);
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

    let url = spawn_test_server_with_config(config).await;
    let resp = reqwest::get(format!("{url}/")).await.unwrap();
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
    let resp = reqwest::get(format!("{url}/")).await.unwrap();
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
    let url = spawn_server(state).await;
    let resp = reqwest::get(format!("{url}/")).await.unwrap();
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
    let url = spawn_test_server().await;
    let resp = reqwest::get(format!("{url}/hooks/nonexistent")).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn hook_detail_renders_hook_config() {
    let config = AppConfig {
        hooks: vec![make_test_hook("Deploy App", "deploy-app", "make deploy")],
        ..AppConfig::default()
    };

    let url = spawn_test_server_with_config(config).await;
    let resp = reqwest::get(format!("{url}/hooks/deploy-app")).await.unwrap();
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

    let url = spawn_test_server_with_config(config).await;
    let resp = reqwest::get(format!("{url}/hooks/disabled-hook")).await.unwrap();
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

    // Test the full page
    let resp = reqwest::get(format!("{url}/hooks/test-hook")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(body.contains("2 total"), "should show total count");
    assert!(
        body.contains("Execution History"),
        "should have execution history section"
    );

    // Test the HTMX partial endpoint
    let resp = reqwest::get(format!("{url}/hooks/test-hook/executions?page=1"))
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
    let url = spawn_test_server().await;
    let resp = reqwest::get(format!("{url}/hooks/nonexistent/executions"))
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

    let url = spawn_test_server_with_config(config).await;

    // The HTMX partial for a hook with no executions
    let resp = reqwest::get(format!("{url}/hooks/empty-hook/executions?page=1"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();

    assert!(
        body.contains("No executions yet"),
        "should show empty state message"
    );
}
