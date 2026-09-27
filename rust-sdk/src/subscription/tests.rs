use super::*;
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_string_contains;
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
        capacity: 10,
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
    for status in [None, None, Some(SwapStatus::Pending), None] {
        emitter.update(id(3), status, StatusSource::Http);
    }
    emitter.reset();
    emitter.update(id(3), None, StatusSource::Http);
    let events: Vec<_> = std::iter::from_fn(|| receiver.try_recv().ok()).collect();
    assert!(matches!(
        events.as_slice(),
        [
            StatusEvent::Missing { .. },
            StatusEvent::Update {
                status: SwapStatus::Pending,
                ..
            },
            StatusEvent::Missing { .. },
            StatusEvent::Missing { .. },
        ]
    ));
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
async fn flapping_peer_backs_off_and_stable_session_resets() {
    let http = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/swap/bulk-status"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"statuses":[],"not_found":[]})),
        )
        .mount(&http)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = SubscriptionOptions {
        reconnect_max: Duration::from_millis(200),
        ..options(&listener)
    };
    let (timings_tx, timings_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut accepted = Vec::new();
        for _ in 0..7 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            accepted.push(Instant::now());
            socket.close(None).await.unwrap();
        }
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let stable_closed = Instant::now();
        socket.close(None).await.unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        timings_tx
            .send((accepted, stable_closed, Instant::now()))
            .unwrap();
        while let Some(Ok(_)) = socket.next().await {}
    });
    let client = Client::new(&http.uri()).unwrap();
    let subscription = client.subscribe_status(config).unwrap();
    subscription.subscribe([id(1)]).unwrap();
    let (accepted, stable_closed, resumed) =
        tokio::time::timeout(Duration::from_secs(5), timings_rx)
            .await
            .unwrap()
            .unwrap();
    // The sixth sleep is 5ms doubled five times, 160ms, unless a bare handshake resets it.
    assert!(accepted[6] - accepted[5] >= Duration::from_millis(100));
    assert!(resumed - stable_closed < Duration::from_millis(100));
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
#[tokio::test]
async fn lag_resends_http_status_already_delivered() {
    let http = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"statuses":[{"id":id(1),"status":"serverfunded"}],"not_found":[]}),
        ))
        .mount(&http)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = options(&listener);
    config.event_capacity = 2;
    config.reconcile_interval = Duration::from_millis(100);
    let (flood_tx, flood_rx) = tokio::sync::oneshot::channel();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        request(&mut socket).await;
        flood_rx.await.unwrap();
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
    next_update(
        &mut subscription,
        StatusSource::Http,
        SwapStatus::ServerFunded,
    )
    .await;
    flood_tx.send(()).unwrap();
    done_rx.await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !matches!(
            subscription.recv().await.unwrap(),
            StatusEvent::Lagged { .. }
        ) {}
    })
    .await
    .unwrap();
    let event = next_update(
        &mut subscription,
        StatusSource::Http,
        SwapStatus::ServerFunded,
    )
    .await;
    assert!(matches!(event, StatusEvent::Update { swap_id, .. } if swap_id == id(1)));
    subscription.shutdown().await.unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn slow_consumer_converges_without_lag() {
    for missing in [false, true] {
        let http = MockServer::start().await;
        let ids: Vec<_> = (0..300).map(id).collect();
        let body = if missing {
            json!({"statuses":[],"not_found":ids})
        } else {
            let statuses: Vec<_> = ids
                .iter()
                .map(|id| json!({"id":id,"status":"serverfunded"}))
                .collect();
            json!({"statuses":statuses,"not_found":[]})
        };
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&http)
            .await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = options(&listener);
        config.event_capacity = 64;
        config.reconcile_interval = Duration::from_millis(100);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            while let Some(Ok(_)) = socket.next().await {}
        });
        let mut subscription = Client::new(&http.uri())
            .unwrap()
            .subscribe_status(config)
            .unwrap();
        subscription.subscribe(ids).unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut seen = std::collections::HashSet::new();
            loop {
                match subscription.recv().await.unwrap() {
                    StatusEvent::Lagged { missed } => panic!("consumer lagged by {missed}"),
                    StatusEvent::Update {
                        swap_id,
                        status: SwapStatus::ServerFunded,
                        source: StatusSource::Http,
                    } if !missing => {
                        seen.insert(swap_id);
                    }
                    StatusEvent::Missing { swap_id } if missing => {
                        seen.insert(swap_id);
                    }
                    StatusEvent::Reconciled if seen.len() == 300 => break,
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        subscription.shutdown().await.unwrap();
        server.abort();
    }
}
#[tokio::test]
// tungstenite's callback signature dictates the large error type.
#[allow(clippy::result_large_err)]
async fn websocket_upgrade_carries_every_protocol_epoch() {
    use tokio_tungstenite::tungstenite::handshake::server::Request;
    let http = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"statuses":[],"not_found":[]})),
        )
        .mount(&http)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = options(&listener);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut headers = None;
        let mut socket =
            tokio_tungstenite::accept_hdr_async(stream, |request: &Request, response| {
                headers = Some(request.headers().clone());
                Ok(response)
            })
            .await
            .unwrap();
        sender.send(headers.unwrap()).unwrap();
        while let Some(Ok(_)) = socket.next().await {}
    });
    let subscription = Client::new(&http.uri())
        .unwrap()
        .subscribe_status(config)
        .unwrap();
    subscription.subscribe([id(1)]).unwrap();
    let headers = tokio::time::timeout(Duration::from_secs(3), receiver)
        .await
        .unwrap()
        .unwrap();
    for (name, value) in crate::client::PROTOCOL_HEADERS {
        assert_eq!(
            headers.get(name).unwrap().to_str().unwrap(),
            value,
            "{name}"
        );
    }
    subscription.shutdown().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn canonical_ids_match_lowercase_replies() {
    let lower = "abcdef00-0000-0000-0000-000000000001".to_string();
    let http = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains(lower.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"statuses":[{"id":lower,"status":"serverfunded"}],"not_found":[]}),
        ))
        .mount(&http)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = options(&listener);
    let expected = lower.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        assert_eq!(request(&mut socket).await["args"], json!([expected]));
        socket
            .send(Message::Text(
                json!({"channel":"swap_status","data":{(expected):"clientfunded"}})
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
    subscription
        .subscribe(["{ABCDEF00-0000-0000-0000-000000000001}".to_string()])
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut websocket, mut http) = (false, false);
        while !(websocket && http) {
            if let StatusEvent::Update {
                swap_id, source, ..
            } = subscription.recv().await.unwrap()
            {
                assert_eq!(swap_id, lower);
                match source {
                    StatusSource::WebSocket => websocket = true,
                    StatusSource::Http => http = true,
                }
            }
        }
    })
    .await
    .unwrap();
    subscription.shutdown().await.unwrap();
    server.await.unwrap();
}
#[tokio::test]
async fn failed_batch_does_not_skip_later_batches() {
    let http = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains(id(0).as_str()))
        .respond_with(ResponseTemplate::new(500).set_delay(Duration::from_millis(200)))
        .with_priority(1)
        .mount(&http)
        .await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(
                    json!({"statuses":[{"id":id(149),"status":"serverfunded"}],"not_found":[]}),
                )
                .set_delay(Duration::from_millis(200)),
        )
        .mount(&http)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = options(&listener);
    config.request_timeout = Duration::from_millis(300);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        while let Some(Ok(_)) = socket.next().await {}
    });
    let mut subscription = Client::new(&http.uri())
        .unwrap()
        .subscribe_status(config)
        .unwrap();
    subscription.subscribe((0..150).map(id)).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        let (mut updated, mut reported) = (false, false);
        while !(updated && reported) {
            match subscription.recv().await.unwrap() {
                StatusEvent::Update {
                    swap_id,
                    source: StatusSource::Http,
                    ..
                } => {
                    assert_eq!(swap_id, id(149));
                    updated = true;
                }
                StatusEvent::Error { message } if message.contains("1 of 2") => reported = true,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    subscription.shutdown().await.unwrap();
    server.abort();
}
