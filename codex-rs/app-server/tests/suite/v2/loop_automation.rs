use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_sequence;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ItemStartedNotification;
use codex_app_server_protocol::LoopCreateParams;
use codex_app_server_protocol::LoopCreateResponse;
use codex_app_server_protocol::LoopDeleteParams;
use codex_app_server_protocol::LoopDeleteResponse;
use codex_app_server_protocol::LoopListParams;
use codex_app_server_protocol::LoopListResponse;
use codex_app_server_protocol::SandboxPolicy;
use codex_app_server_protocol::ThreadItem;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput as V2UserInput;
use codex_features::Feature;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::Duration;
use tokio::time::timeout;

const MONITOR_TEST_TIMEOUT: Duration = Duration::from_secs(15);

#[tokio::test]
async fn loop_rpc_create_list_and_delete_share_durable_thread_state() -> Result<()> {
    let responses = create_mock_responses_server_sequence(Vec::new()).await;
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(&responses.uri()).write(codex_home.path())?;
    let mut app_server = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;
    let thread_request_id = app_server
        .send_thread_start_request_with_auto_env(ThreadStartParams::default())
        .await?;
    let ThreadStartResponse { thread, .. } = app_server.read_response(thread_request_id).await?;

    let LoopCreateResponse { loop_: created } = app_server
        .request(|request_id| ClientRequest::LoopCreate {
            request_id,
            params: LoopCreateParams {
                thread_id: thread.id.clone(),
                name: Some("fixture-review".to_string()),
                prompt: "Review only the disposable fixture.".to_string(),
                every_seconds: 900,
            },
        })
        .await?;
    assert_eq!(created.name, "fixture-review");
    assert_eq!(created.prompt, "Review only the disposable fixture.");
    assert_eq!(created.every_seconds, 900);
    assert!(created.enabled);

    let LoopListResponse { data, next_cursor } = app_server
        .request(|request_id| ClientRequest::LoopList {
            request_id,
            params: LoopListParams {
                thread_id: thread.id.clone(),
                cursor: None,
                limit: None,
            },
        })
        .await?;
    assert_eq!(data, vec![created.clone()]);
    assert_eq!(next_cursor, None);

    let LoopDeleteResponse { loop_: deleted } = app_server
        .request(|request_id| ClientRequest::LoopDelete {
            request_id,
            params: LoopDeleteParams {
                thread_id: thread.id.clone(),
                id: created.id.clone(),
            },
        })
        .await?;
    assert_eq!(deleted, created);
    let LoopListResponse { data, next_cursor } = app_server
        .request(|request_id| ClientRequest::LoopList {
            request_id,
            params: LoopListParams {
                thread_id: thread.id,
                cursor: None,
                limit: Some(1),
            },
        })
        .await?;
    assert_eq!(data, Vec::new());
    assert_eq!(next_cursor, None);
    Ok(())
}

#[tokio::test]
#[cfg_attr(windows, ignore = "fixture command uses a POSIX shell")]
async fn monitor_wakes_idle_thread_for_matching_background_output() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let command_args = json!({
        "cmd": "sleep 2; printf NATIVE_MONITOR_READY; sleep 2",
        "yield_time_ms": 250
    });
    let command_responses = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("command-response"),
                responses::ev_function_call(
                    "command-call",
                    "exec_command",
                    &command_args.to_string(),
                ),
                responses::ev_completed("command-response"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("command-completion"),
                responses::ev_assistant_message("command-message", "command started"),
                responses::ev_completed("command-completion"),
            ]),
        ],
    )
    .await;
    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .with_sandbox_mode("danger-full-access")
        .enable_feature(Feature::UnifiedExec)
        .write(codex_home.path())?;
    let mut app_server = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;
    let ThreadStartResponse { thread, .. } = app_server
        .start_thread(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;

    let _: TurnStartResponse = app_server
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![V2UserInput::Text {
                    text: "Start the monitor fixture process.".to_string(),
                    text_elements: Vec::new(),
                }],
                sandbox_policy: Some(SandboxPolicy::DangerFullAccess),
                ..Default::default()
            },
        })
        .await?;
    let process_id = timeout(MONITOR_TEST_TIMEOUT, async {
        loop {
            let notification: ItemStartedNotification =
                app_server.read_notification("item/started").await?;
            if let ThreadItem::CommandExecution {
                process_id: Some(process_id),
                ..
            } = notification.item
            {
                return Ok::<String, anyhow::Error>(process_id);
            }
        }
    })
    .await??;
    timeout(
        MONITOR_TEST_TIMEOUT,
        app_server.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    assert_eq!(command_responses.requests().len(), 2);

    let monitor_args = json!({
        "process_id": process_id.parse::<i32>()?,
        "prompt": "Report the fixture marker.",
        "name": "native-monitor-fixture",
        "contains": "NATIVE_MONITOR_READY",
        "poll_seconds": 1,
        "once": true
    });
    let monitor_responses = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("monitor-response"),
                responses::ev_function_call(
                    "monitor-call",
                    "monitor_start",
                    &monitor_args.to_string(),
                ),
                responses::ev_completed("monitor-response"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("monitor-completion"),
                responses::ev_assistant_message("monitor-message", "monitor armed"),
                responses::ev_completed("monitor-completion"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("monitor-wakeup"),
                responses::ev_assistant_message("monitor-wakeup-message", "marker observed"),
                responses::ev_completed("monitor-wakeup"),
            ]),
        ],
    )
    .await;

    let _: TurnStartResponse = app_server
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id,
                input: vec![V2UserInput::Text {
                    text: "Attach the native monitor.".to_string(),
                    text_elements: Vec::new(),
                }],
                sandbox_policy: Some(SandboxPolicy::DangerFullAccess),
                ..Default::default()
            },
        })
        .await?;
    timeout(
        MONITOR_TEST_TIMEOUT,
        app_server.read_stream_until_notification_message("turn/completed"),
    )
    .await??;
    timeout(
        MONITOR_TEST_TIMEOUT,
        app_server.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let requests = monitor_responses.requests();
    assert_eq!(requests.len(), 3);
    let wakeup_input = requests[2].input();
    assert!(
        wakeup_input.iter().any(|item| {
            item.to_string().contains("[MONITOR native-monitor-fixture")
                && item.to_string().contains("NATIVE_MONITOR_READY")
        }),
        "automatic turn should contain the monitor event: {wakeup_input:?}"
    );
    Ok(())
}
