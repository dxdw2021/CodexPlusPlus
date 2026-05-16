use codex_plus_core::assets;
use codex_plus_core::bridge::{self, BRIDGE_BINDING_NAME};
use codex_plus_core::cdp::{CdpTarget, pick_page_target};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

fn target(id: &str, kind: &str, title: &str, url: &str, websocket_url: Option<&str>) -> CdpTarget {
    CdpTarget {
        id: id.to_string(),
        target_type: kind.to_string(),
        title: title.to_string(),
        url: url.to_string(),
        web_socket_debugger_url: websocket_url.map(str::to_string),
    }
}

#[test]
fn bridge_script_defines_expected_globals_and_binding() {
    let script = bridge::build_bridge_script(BRIDGE_BINDING_NAME);

    assert!(script.contains("window.__codexSessionDeleteBridge"));
    assert!(script.contains("window.__codexSessionDeleteResolve"));
    assert!(script.contains("window.__codexSessionDeleteReject"));
    assert!(script.contains("codexSessionDeleteV2"));
}

#[test]
fn injection_script_prefixes_helper_url_and_sponsor_images() {
    let script = assets::injection_script(57321);

    assert!(script.contains("window.__CODEX_SESSION_DELETE_HELPER__"));
    assert!(script.contains("http://127.0.0.1:57321"));
    assert!(script.contains("window.__CODEX_PLUS_SPONSOR_IMAGES__"));
}

#[test]
fn cdp_target_deserializes_websocket_field() {
    let target: CdpTarget = serde_json::from_value(json!({
        "id": "page-1",
        "type": "page",
        "title": "Codex",
        "url": "https://codex.test",
        "webSocketDebuggerUrl": "ws://debug",
    }))
    .expect("target should deserialize");

    assert_eq!(target.target_type, "page");
    assert_eq!(
        target.web_socket_debugger_url.as_deref(),
        Some("ws://debug")
    );
}

#[test]
fn runtime_evaluate_params_matches_python_flags() {
    let params = bridge::runtime_evaluate_params("1 + 1");

    assert_eq!(params["expression"], "1 + 1");
    assert_eq!(params["awaitPromise"], false);
    assert_eq!(params["allowUnsafeEvalBlockedByCSP"], true);
}

#[test]
fn bridge_result_expressions_json_escape_inputs() {
    let resolve = bridge::resolve_bridge_expression("request\"1", &json!({"status": "ok"}))
        .expect("resolve expression should build");
    let reject = bridge::reject_bridge_expression("request\"1", "bad \"value\"")
        .expect("reject expression should build");

    assert_eq!(
        resolve,
        r#"window.__codexSessionDeleteResolve("request\"1", {"status":"ok"})"#
    );
    assert_eq!(
        reject,
        r#"window.__codexSessionDeleteReject("request\"1", "bad \"value\"")"#
    );
}

#[test]
fn pick_page_target_prefers_codex_title_or_url() {
    let targets = vec![
        target(
            "first",
            "page",
            "Other",
            "https://example.test",
            Some("ws://first"),
        ),
        target(
            "second",
            "page",
            "Codex",
            "https://example.test",
            Some("ws://second"),
        ),
        target(
            "third",
            "page",
            "Other",
            "https://codex.test",
            Some("ws://third"),
        ),
    ];

    let picked = pick_page_target(&targets).expect("target should be selected");

    assert_eq!(picked.id, "second");
}

#[test]
fn pick_page_target_falls_back_to_first_injectable_page() {
    let targets = vec![
        target(
            "browser",
            "browser",
            "Codex",
            "https://codex.test",
            Some("ws://browser"),
        ),
        target(
            "first",
            "page",
            "Other",
            "https://example.test",
            Some("ws://first"),
        ),
        target(
            "second",
            "page",
            "Other 2",
            "https://example.test/2",
            Some("ws://second"),
        ),
    ];

    let picked = pick_page_target(&targets).expect("target should be selected");

    assert_eq!(picked.id, "first");
}

#[test]
fn pick_page_target_rejects_non_pages_and_pages_without_websocket() {
    let targets = vec![
        target(
            "browser",
            "browser",
            "Codex",
            "https://codex.test",
            Some("ws://browser"),
        ),
        target("page-no-ws", "page", "Codex", "https://codex.test", None),
    ];

    let error = pick_page_target(&targets).expect_err("no injectable page should be selected");

    assert!(
        error
            .to_string()
            .contains("No injectable Codex page target found")
    );
}

#[tokio::test]
async fn install_bridge_routes_binding_while_waiting_for_command_response() {
    let (url, request_rx) = spawn_cdp_server(|mut socket| async move {
        for expected_id in 1..=4 {
            let command = recv_json(&mut socket).await;
            assert_eq!(command["id"], expected_id);
            send_json(&mut socket, json!({ "id": expected_id, "result": {} })).await;
        }

        let evaluate = recv_json(&mut socket).await;
        assert_eq!(evaluate["id"], 5);
        assert_eq!(evaluate["method"], "Runtime.evaluate");
        send_json(
            &mut socket,
            json!({
                "method": "Runtime.bindingCalled",
                "params": {
                    "payload": serde_json::to_string(&json!({
                        "id": "request-1",
                        "path": "delete",
                        "payload": { "target": "session" },
                    })).unwrap(),
                },
            }),
        )
        .await;
        send_json(&mut socket, json!({ "id": 5, "result": {} })).await;

        let response = recv_json(&mut socket).await;
        assert_eq!(response["method"], "Runtime.evaluate");
        assert!(
            response["params"]["expression"]
                .as_str()
                .expect("expression should be string")
                .contains("__codexSessionDeleteResolve")
        );
        send_json(&mut socket, json!({ "id": response["id"], "result": {} })).await;
        close_socket(&mut socket).await;
    })
    .await;

    let handled = Arc::new(AtomicBool::new(false));
    let handler = {
        let handled = Arc::clone(&handled);
        Arc::new(move |path: String, payload: serde_json::Value| {
            let handled = Arc::clone(&handled);
            Box::pin(async move {
                assert_eq!(path, "delete");
                assert_eq!(payload["target"], "session");
                handled.store(true, Ordering::SeqCst);
                Ok(json!({ "status": "ok" }))
            })
                as Pin<Box<dyn Future<Output = anyhow::Result<serde_json::Value>> + Send>>
        })
    };

    tokio::time::timeout(
        Duration::from_secs(2),
        bridge::install_bridge(&url, BRIDGE_BINDING_NAME, handler, &[]),
    )
    .await
    .expect("bridge should not hang while processing interleaved binding call")
    .expect("bridge should keep processing interleaved binding call");
    request_rx
        .await
        .expect("server task should finish without panicking");
    assert!(handled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn install_bridge_command_error_mentions_method_and_id() {
    let (url, request_rx) = spawn_cdp_server(|mut socket| async move {
        let command = recv_json(&mut socket).await;
        assert_eq!(command["method"], "Runtime.enable");
        send_json(
            &mut socket,
            json!({
                "id": command["id"],
                "error": { "code": -32000, "message": "Runtime disabled" },
            }),
        )
        .await;
        close_socket(&mut socket).await;
    })
    .await;

    let handler = noop_handler();
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        bridge::install_bridge(&url, BRIDGE_BINDING_NAME, handler, &[]),
    )
    .await
    .expect("bridge should not hang on CDP error response")
    .expect_err("CDP error response should fail install");
    let message = error.to_string();

    request_rx
        .await
        .expect("server task should finish without panicking");
    assert!(message.contains("Runtime.enable"), "{message}");
    assert!(message.contains("id 1"), "{message}");
    assert!(message.contains("Runtime disabled"), "{message}");
}

#[tokio::test]
async fn install_bridge_rejects_bad_payload_with_id_and_continues_after_unparseable_payload() {
    let (url, request_rx) = spawn_cdp_server(|mut socket| async move {
        for expected_id in 1..=5 {
            let command = recv_json(&mut socket).await;
            assert_eq!(command["id"], expected_id);
            send_json(&mut socket, json!({ "id": expected_id, "result": {} })).await;
        }

        send_json(
            &mut socket,
            json!({
                "method": "Runtime.bindingCalled",
                "params": { "payload": "{\"id\":\"bad-1\",\"payload\":{}" },
            }),
        )
        .await;
        send_json(
            &mut socket,
            json!({
                "method": "Runtime.bindingCalled",
                "params": { "payload": "not json" },
            }),
        )
        .await;
        send_json(
            &mut socket,
            json!({
                "method": "Runtime.bindingCalled",
                "params": {
                    "payload": serde_json::to_string(&json!({
                        "id": "ok-1",
                        "path": "delete",
                        "payload": {},
                    })).unwrap(),
                },
            }),
        )
        .await;

        let reject = recv_json(&mut socket).await;
        assert!(
            reject["params"]["expression"]
                .as_str()
                .expect("expression should be string")
                .contains("__codexSessionDeleteReject")
        );
        assert!(
            reject["params"]["expression"]
                .as_str()
                .expect("expression should be string")
                .contains("bad-1")
        );
        send_json(&mut socket, json!({ "id": reject["id"], "result": {} })).await;

        let resolve = recv_json(&mut socket).await;
        assert!(
            resolve["params"]["expression"]
                .as_str()
                .expect("expression should be string")
                .contains("__codexSessionDeleteResolve")
        );
        assert!(
            resolve["params"]["expression"]
                .as_str()
                .expect("expression should be string")
                .contains("ok-1")
        );
        send_json(&mut socket, json!({ "id": resolve["id"], "result": {} })).await;
        close_socket(&mut socket).await;
    })
    .await;

    tokio::time::timeout(
        Duration::from_secs(2),
        bridge::install_bridge(&url, BRIDGE_BINDING_NAME, noop_handler(), &[]),
    )
    .await
    .expect("bridge should not hang after bad payload")
    .expect("bad payloads should not terminate the bridge loop");
    request_rx
        .await
        .expect("server task should finish without panicking");
}

type TestSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn spawn_cdp_server<F, Fut>(handler: F) -> (String, oneshot::Receiver<()>)
where
    F: FnOnce(TestSocket) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener should bind");
    let address = listener.local_addr().expect("listener should have address");
    let (done_tx, done_rx) = oneshot::channel();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("client should connect");
        let socket = accept_async(stream)
            .await
            .expect("websocket should upgrade");
        handler(socket).await;
        let _ = done_tx.send(());
    });

    (websocket_url(address), done_rx)
}

fn websocket_url(address: SocketAddr) -> String {
    format!("ws://{address}")
}

async fn recv_json(socket: &mut TestSocket) -> serde_json::Value {
    let message = socket
        .next()
        .await
        .expect("client should send message")
        .expect("message should be readable");
    let Message::Text(text) = message else {
        panic!("expected text websocket message");
    };
    serde_json::from_str(&text).expect("message should be JSON")
}

async fn send_json(socket: &mut TestSocket, value: serde_json::Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("message should send");
}

async fn close_socket(socket: &mut TestSocket) {
    socket.close(None).await.expect("websocket should close");
    let _ = tokio::time::timeout(Duration::from_millis(200), socket.next()).await;
}

fn noop_handler() -> bridge::BridgeHandler {
    Arc::new(|_, _| {
        Box::pin(async { Ok(json!({ "status": "ok" })) })
            as Pin<Box<dyn Future<Output = anyhow::Result<serde_json::Value>> + Send>>
    })
}
