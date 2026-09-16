use super::*;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn id(index: usize) -> String {
    format!("00000000-0000-0000-0000-{index:012}")
}
fn options(socket: &TcpListener) -> SubscriptionOptions {
    SubscriptionOptions {
        websocket_url: Some(
            format!("ws://{}/ws", socket.local_addr().unwrap())
                .parse()
                .unwrap(),
        ),
        reconnect_min: Duration::from_millis(5),
        reconnect_max: Duration::from_millis(20),
        request_timeout: Duration::from_secs(2),
        ..Default::default()
    }
}
async fn next_update(
    subscription: &mut StatusSubscription,
    source: StatusSource,
    status: SwapStatus,
) -> StatusEvent {
    tokio::time::timeout(Duration::from_secs(3),async {
        loop {let event=subscription.recv().await.unwrap();if matches!(&event,StatusEvent::Update {source:got_source,status:got_status,..} if *got_source==source && *got_status==status) {return event;}}
    }).await.unwrap()
}
async fn request(socket: &mut WebSocketStream<tokio::net::TcpStream>) -> Value {
    loop {
        if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
            return serde_json::from_str(&text).unwrap();
        }
    }
}
#[test]
fn urls_protocol_validation_deduplication_and_regressions() {
    assert_eq!(
        websocket_url(&Url::parse("https://example.org/api/?x=1#fragment").unwrap())
            .unwrap()
            .as_str(),
        "wss://example.org/api/ws"
    );
    let (events, mut receiver) = broadcast::channel(10);
    let emitter = Emitter {
        events,
        last: Mutex::new(HashMap::new()),
    };
    let active = vec![id(1)];
    process_message("no json", &active, &emitter);
    assert!(matches!(
        receiver.try_recv().unwrap(),
        StatusEvent::Error { .. }
    ));
    process_message(
        &json!({"channel":"other","data":{(id(1)):"clientfunded"}}).to_string(),
        &active,
        &emitter,
    );
    process_message(
        &json!({"channel":"swap_status","data":{(id(2)):"clientfunded"}}).to_string(),
        &active,
        &emitter,
    );
    assert!(receiver.try_recv().is_err());
    for status in ["clientfunded", "clientfunded", "pending"] {
        process_message(
            &json!({"channel":"swap_status","data":{(id(1)):status}}).to_string(),
            &active,
            &emitter,
        );
    }
    assert!(matches!(
        receiver.try_recv().unwrap(),
        StatusEvent::Update {
            status: SwapStatus::ClientFunded,
            ..
        }
    ));
    assert!(matches!(
        receiver.try_recv().unwrap(),
        StatusEvent::Update {
            status: SwapStatus::Pending,
            ..
        }
    ));
    assert!(receiver.try_recv().is_err());
}
#[tokio::test]
async fn reconnect_resubscribes_and_http_reconciles_with_cancellation() {
    let http = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/bulk-status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"statuses":[{"id":id(1),"status":"serverfunded"}],"not_found":[]}),
        ))
        .mount(&http)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = options(&listener);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        assert_eq!(request(&mut socket).await["args"], json!([id(1)]));
        socket
            .send(Message::Text(
                json!({"channel":"swap_status","data":{(id(1)):"clientfunded"}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        socket.close(None).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        assert_eq!(request(&mut socket).await["args"], json!([id(1)]));
        socket
            .send(Message::Text(
                json!({"channel":"swap_status","data":{(id(1)):"pending"}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        while let Some(Ok(_)) = socket.next().await {}
    });
    let client = Client::new(&http.uri()).unwrap();
    let mut subscription = client.subscribe_status(config).unwrap();
    subscription.subscribe([id(1)]).unwrap();
    // Capture interleaved HTTP and socket events without assuming their order.
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut funded, mut pending, mut reconciled, mut reconnected) =
            (false, false, false, false);
        while !(funded && pending && reconciled && reconnected) {
            match subscription.recv().await.unwrap() {
                StatusEvent::Update {
                    status: SwapStatus::ClientFunded,
                    source: StatusSource::WebSocket,
                    ..
                } => funded = true,
                StatusEvent::Update {
                    status: SwapStatus::Pending,
                    source: StatusSource::WebSocket,
                    ..
                } => pending = true,
                StatusEvent::Update {
                    status: SwapStatus::ServerFunded,
                    source: StatusSource::Http,
                    ..
                } => reconciled = true,
                StatusEvent::Connected { reconnected: true } => reconnected = true,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    subscription.shutdown().await.unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn subscription_caps_batches_and_promotes_queued_ids() {
    let http = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"statuses":[],"not_found":[]})),
        )
        .mount(&http)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = options(&listener);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        for batch in 0..4 {
            let frame = request(&mut socket).await;
            assert_eq!(frame["op"], "subscribe");
            assert_eq!(frame["args"].as_array().unwrap().len(), 64);
            assert_eq!(frame["args"][0], id(batch * 64));
        }
        ready_tx.send(()).unwrap();
        let frame = request(&mut socket).await;
        assert_eq!(frame["op"], "unsubscribe");
        assert_eq!(frame["args"], json!([id(0)]));
        let frame = request(&mut socket).await;
        assert_eq!(frame["op"], "subscribe");
        assert_eq!(frame["args"], json!([id(256)]));
        socket
            .send(Message::Text(
                json!({"channel":"swap_status","data":{(id(256)):"clientfunded"}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut subscription = Client::new(&http.uri())
        .unwrap()
        .subscribe_status(config)
        .unwrap();
    subscription.subscribe((0..300).map(id)).unwrap();
    ready_rx.await.unwrap();
    subscription.unsubscribe(&[id(0)]);
    let event = next_update(
        &mut subscription,
        StatusSource::WebSocket,
        SwapStatus::ClientFunded,
    )
    .await;
    assert!(matches!(event,StatusEvent::Update {swap_id,..} if swap_id==id(256)));
    subscription.shutdown().await.unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn lag_is_reported_and_shutdown_cancels_stalled_http_and_handshake() {
    let http = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
        .mount(&http)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = options(&listener);
    config.event_capacity = 2;
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        request(&mut socket).await;
        for index in 0..100 {
            socket.send(Message::Text(json!({"channel":"swap_status","data":{(id(1)):format!("future_status_{index}")}}).to_string().into())).await.unwrap();
        }
        done_tx.send(()).unwrap();
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut subscription = Client::new(&http.uri())
        .unwrap()
        .subscribe_status(config)
        .unwrap();
    subscription.subscribe([id(1)]).unwrap();
    done_rx.await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(matches!(
        subscription.recv().await,
        Some(StatusEvent::Lagged { .. })
    ));
    tokio::time::timeout(Duration::from_millis(200), subscription.shutdown())
        .await
        .unwrap()
        .unwrap();
    server.await.unwrap();
    // A peer that accepts TCP but never completes the WebSocket handshake.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = options(&listener);
    let subscription = Client::new(&http.uri())
        .unwrap()
        .subscribe_status(config)
        .unwrap();
    let (_socket, _) = listener.accept().await.unwrap();
    tokio::time::timeout(Duration::from_millis(200), subscription.shutdown())
        .await
        .unwrap()
        .unwrap();
}
