use sendword::config::AppConfig;
use sendword::db::Db;
use sendword::server::AppState;
use sendword::templates::Templates;

async fn spawn_test_server() -> String {
    let config = AppConfig::default();
    let db = Db::new_in_memory().await.expect("in-memory db");
    db.migrate().await.expect("migration");
    let templates = Templates::new(Templates::default_dir());
    let state = AppState::new(config, db, templates);

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

    let resp = client.post(format!("{url}/hook/test-hook")).send().await.unwrap();
    assert_eq!(resp.status(), 501);

    let resp = client.get(format!("{url}/hooks/test-hook")).send().await.unwrap();
    assert_eq!(resp.status(), 501);

    let resp = client.get(format!("{url}/executions/some-id")).send().await.unwrap();
    assert_eq!(resp.status(), 501);

    let resp = client
        .post(format!("{url}/executions/some-id/replay"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 501);
}
