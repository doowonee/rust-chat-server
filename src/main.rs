//! Example chat application.
//!
//! Run with
//!
//! ```not_rust
//! cd examples && cargo run -p example-chat
//! ```

mod session;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Extension,
    },
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use fred::{
    clients::SubscriberClient,
    prelude::*,
    types::{ReconnectPolicy, RedisConfig},
};
use futures::{
    sink::SinkExt,
    stream::{SplitSink, StreamExt},
    FutureExt,
};
use std::{
    collections::{BTreeMap, HashSet},
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// Our shared state
struct AppState {
    user_set: Mutex<HashSet<String>>,
    /// 세션 아이디를 키로 하고 웹소켓 송신 스트림을 값으로 하는 트리맵
    websocket_sessions: Mutex<BTreeMap<String, SplitSink<WebSocket, Message>>>,
    // /// 세션 아이디를 키 유저 아디 정보를 담은 트리맵
    // websocket_sessions: Mutex<BTreeMap<String, String>>,
    tx: broadcast::Sender<String>,
    redis_client: RedisClient,
    redis_subscriber: SubscriberClient,
}

/// pub sub 대상 채널 이름
const REDIS_CHANNEL_NAME: &str = "zxc";

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 서버 내부 메모리 에서 유저풀 관리
    let user_set = Mutex::new(HashSet::new());
    let (tx, _rx) = broadcast::channel(100);

    // Redis 클라이언트 준비
    let config = RedisConfig::default();
    let policy = ReconnectPolicy::default();
    let redis_client = RedisClient::new(config.clone());
    // connect to the server, returning a handle to the task that drives the connection
    let _ = redis_client.connect(Some(policy.clone()));
    let _ = redis_client.wait_for_connect().await.unwrap();

    // fred SubscriberClient 사용해서 redis subscribe 자동 재연결 처리
    let redis_subscriber = SubscriberClient::new(config);
    let _ = redis_subscriber.connect(Some(policy));
    let _ = redis_subscriber.wait_for_connect().await.unwrap();
    let _ = redis_subscriber.manage_subscriptions();
    let _ = redis_subscriber
        .subscribe(REDIS_CHANNEL_NAME)
        .await
        .unwrap();
    tracing::info!("redis subscribe 완료");

    let websocket_sessions = Mutex::new(BTreeMap::new());
    // 웹 서버 핸들러간 공유
    let app_state = Arc::new(AppState {
        user_set,
        websocket_sessions,
        tx,
        redis_client,
        redis_subscriber,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/websocket", get(websocket_handler))
        .layer(Extension(app_state));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    tracing::debug!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| websocket(socket, state, session::generate_id()))
}

async fn websocket(stream: WebSocket, state: Arc<AppState>, sid: String) {
    tracing::debug!("{} 연결됌", sid);
    // By splitting we can send and receive at the same time.
    let (mut sender, mut receiver) = stream.split();

    // Use an unbounded channel to handle buffering and flushing of messages
    // to the websocket...K
    let (ub_tx, ub_rx) = mpsc::unbounded_channel();
    // tokio 의 unbound 채널을 future의 Stream trait 을 지원하는 스트림으로 변환 한다.
    let ub_rx = UnboundedReceiverStream::new(ub_rx);
    // tokio 비동기 태스크로 future의 스트림을 웹소켓 송신 채널로 포워딩 해준다
    tokio::spawn(ub_rx.forward(sender).map(move |result| {
        if let Err(e) = result {
            // Connection closed normally 라고 찍힘
            tracing::error!("websocket send error: {}", e);
            // cloned_disconnected_event_tx.send(()).unwrap();
        }
    }));

    // Username gets set in the receive loop, if it's valid.
    let mut username = String::new();
    // Loop until a text message is found.
    while let Some(Ok(message)) = receiver.next().await {
        if let Message::Text(name) = message {
            // If username that is sent by client is not taken, fill username string.
            check_username(&state, &mut username, &name);

            // If not empty we want to quit the loop else we want to quit function.
            if !username.is_empty() {
                break;
            } else {
                // Only send our client that username is taken.
                let _ = ub_tx.send(Ok(Message::Text(String::from("Username already taken."))));
                return;
            }
        }
    }

    // Subscribe before sending joined message.
    let mut rx = state.tx.subscribe();

    // Send joined message to all subscribers.
    let msg = format!("{} joined.", username);
    tracing::debug!("{}", msg);
    let redis_client = state.redis_client.clone();
    // @TODO unwrap 에러시 몇번 재시도 하고 에러로 취급
    let received_clients: i64 = redis_client
        .publish(REDIS_CHANNEL_NAME, &msg)
        .await
        .unwrap();
    tracing::debug!("redis publish: {}", received_clients);
    // let _ = state.tx.send(msg);

    // This task will receive broadcast messages and send text message to our client.
    // let mut send_task = tokio::spawn(async move {
    //     while let Ok(msg) = rx.recv().await {
    //         // In any websocket error, break loop.
    //         if sender.send(Message::Text(msg)).await.is_err() {
    //             break;
    //         }
    //     }
    // });

    let redis_subscriber = state.redis_subscriber.clone();
    // redis subscribe 한 데이터를 웹소켓으로 전송
    let mut send_task = tokio::spawn(async move {
        let mut message_stream = redis_subscriber.on_message();
        while let Some((channel, message)) = message_stream.next().await {
            tracing::debug!("Recv {:?} on channel {}", message, channel);
            if ub_tx
                .send(Ok(Message::Text(message.as_string().unwrap_or_default())))
                .is_err()
            {
                break;
            }
            // if sender
            //     .send(Message::Text(message.as_string().unwrap_or_default()))
            //     .await
            //     .is_err()
            // {
            //     break;
            // }
        }
        Ok::<_, RedisError>(())
    });

    // let mut send_task = tokio::spawn(redis_subscriber.on_message().for_each(
    //     |(channel, message)| {
    //         println!("Recv {:?} on channel {}", message, channel);
    //         Ok(())
    //     },
    // ));

    // tracing::debug!("Recv {:?} on channel {}", message, channel);
    // Clone things we want to pass to the receiving task.
    let tx = state.tx.clone();
    let redis_client = state.redis_client.clone();
    let name = username.clone();

    // This task will receive messages from client and send them to broadcast subscribers.
    // 웹소켓으로 부터 데이터를 수신하면 redis로 publish
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(Message::Text(text))) = receiver.next().await {
            // Add username before message.
            let msg = format!("{}: {}", name, text);
            tracing::debug!("redis에 전달 할거 {}", msg);
            // @TODO unwrap 에러시 몇번 재시도 하고 에러로 취급
            let received_clients: i64 = redis_client
                .publish(REDIS_CHANNEL_NAME, &msg)
                .await
                .unwrap();
            tracing::debug!("redis publish: {}", received_clients);
            // 서버 내에서 송신 채널에는 데이터 전송 안함
            // let _ = tx.send(msg);
        }
    });

    // If any one of the tasks exit, abort the other.
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };

    // Send user left message.
    let msg = format!("{} left.", username);
    tracing::debug!("{}", msg);
    let redis_client = state.redis_client.clone();
    // @TODO unwrap 에러시 몇번 재시도 하고 에러로 취급
    let received_clients: i64 = redis_client
        .publish(REDIS_CHANNEL_NAME, &msg)
        .await
        .unwrap();
    tracing::debug!("redis publish: {}", received_clients);
    // let _ = state.tx.send(msg);

    // Remove username from map so new clients can take it.
    state.user_set.lock().unwrap().remove(&username);
}

fn check_username(state: &AppState, string: &mut String, name: &str) {
    let mut user_set = state.user_set.lock().unwrap();

    if !user_set.contains(name) {
        user_set.insert(name.to_owned());

        string.push_str(name);
    }
}

// Include utf-8 file at **compile** time.
async fn index() -> Html<&'static str> {
    Html(include_str!("../static/chat.html"))
}
