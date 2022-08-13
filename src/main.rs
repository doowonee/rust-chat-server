//! Example chat application.
//!
//! Run with
//!
//! ```not_rust
//! cd examples && cargo run -p example-chat
//! ```

mod error;
mod protocol;
mod session;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Extension,
    },
    response::{Html, IntoResponse},
    routing::get,
    Error, Router,
};
use chrono::prelude::*;
use fred::{
    clients::SubscriberClient,
    prelude::*,
    types::{ReconnectPolicy, RedisConfig},
};
use futures::{stream::StreamExt, FutureExt};
use session::Session;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::protocol::{AuthRequest, PacketKind, Ping, Request, SendTextRequest};

// Our shared state
pub struct AppState {
    /// 세션 아이디를 키로 하고 세션 구조체를 값으로 하는 맵
    sessions: Mutex<BTreeMap<String, Session>>,
    /// 유저 아이디를 키로하고 세션 아이디를 값으로 하는 트리맵
    users: Mutex<BTreeMap<String, String>>,
    redis_client: RedisClient,
    redis_subscriber: SubscriberClient,
}

/// pub sub 대상 채널 이름
pub const REDIS_CHANNEL_NAME: &str = "zxc";

/// ubounded channel 로 웹소켓 송신 채널로 포워딩 된다
pub type WebsocketTx = UnboundedSender<Result<Message, Error>>;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

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

    let sessions = Mutex::new(BTreeMap::new());
    let users = Mutex::new(BTreeMap::new());
    // 웹 서버 핸들러간 공유
    let app_state = Arc::new(AppState {
        sessions,
        users,
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
    tracing::debug!("{} 연결 수립", sid);
    // By splitting we can send and receive at the same time.
    let (sender, mut receiver) = stream.split();

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

    // 세션 정보에 웹소켓 송신 스트림 정보를 넣는다
    session::add_session(&state, &sid, ub_tx.clone());

    tracing::debug!("redis subscribe 준비");
    let redis_subscriber = state.redis_subscriber.clone();
    let cloned_tx = ub_tx.clone();
    let cloned_sid = sid.clone();
    // redis subscribe 한 데이터를 웹소켓으로 전송
    let mut send_task = tokio::spawn(async move {
        let mut message_stream = redis_subscriber.on_message();
        while let Some((channel, message)) = message_stream.next().await {
            tracing::debug!(
                "세션 {} 수신 {:?} on channel {}",
                cloned_sid,
                message,
                channel
            );
            // @TODO 레디스로 받은 메시지는 전체 전파 용이므로 일단 전체 전파 나중에 채팅방 개념 생기면 그때 필터링
            if cloned_tx
                .send(Ok(Message::Text(message.as_string().unwrap_or_default())))
                .is_err()
            {
                break;
            }
        }
        Ok::<_, RedisError>(())
    });
    tracing::debug!("redis subscribe 블록 벗어남");

    // This task will receive messages from client and send them to broadcast subscribers.
    // 웹소켓으로 부터 데이터를 수신하면 redis로 publish
    let cloned_sid = sid.clone();
    let cloned_state = state.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(message)) = receiver.next().await {
            if let Message::Text(payload) = message {
                let now = Utc::now().naive_utc();
                tracing::debug!("수신 패킷 {}", payload);
                // return statement means close the connection

                // check packet format
                let packet: Request = match serde_json::from_str(&payload) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!("Wrong JSON: {} payload: {}", e, payload);
                        return;
                    }
                };

                // handel only request op code
                match packet.op_code {
                    PacketKind::Ping => {
                        let req: Ping = match serde_json::from_value(packet.payload) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::error!("Wrong Ping: {} payload: {}", e, payload);
                                return;
                            }
                        };

                        req.handle(sid.clone(), ub_tx.clone(), now).await;
                    }
                    PacketKind::AuthRequest => {
                        let req: AuthRequest = match serde_json::from_value(packet.payload) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::error!("Wrong AuthRequest: {} payload: {}", e, payload);
                                return;
                            }
                        };

                        req.handle(sid.clone(), ub_tx.clone(), state.clone()).await;
                    }
                    PacketKind::SendTextRequest => {
                        let req: SendTextRequest = match serde_json::from_value(packet.payload) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::error!(
                                    "Wrong SendTextRequest: {} payload: {}",
                                    e,
                                    payload
                                );
                                return;
                            }
                        };

                        req.handle(sid.clone(), ub_tx.clone(), state.clone()).await;
                    }
                    _ => {
                        tracing::error!("{:?} Wrong Request {}", packet.op_code, payload);
                        return;
                    }
                };
            }
        }
    });

    // If any one of the tasks exit, abort the other.
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };

    // Send user left message.
    tracing::debug!("{} 연결 종료 감지", cloned_sid);
    // 인증된 세션이면 유저 정보 삭제 그리고 세션 정보 삭제
    let mut sessions = cloned_state.sessions.lock().unwrap();
    match sessions.get(&cloned_sid) {
        Some(v) => {
            let mut users = cloned_state.users.lock().unwrap();
            users.remove(&v.user().map(|v| v.id.clone()).unwrap_or_default());
            sessions.remove(&cloned_sid);
            tracing::debug!("{} 세션 제거 완료", cloned_sid);
        }
        None => {
            tracing::debug!("{} 인증 안한 세션이라 그냥 종료", cloned_sid);
        }
    };

    // let msg = format!("{} left.", username);
    // tracing::debug!("{}", msg);
    // let redis_client = state.redis_client.clone();
    // // @TODO unwrap 에러시 몇번 재시도 하고 에러로 취급
    // let received_clients: i64 = redis_client
    //     .publish(REDIS_CHANNEL_NAME, &msg)
    //     .await
    //     .unwrap();
    // tracing::debug!("redis publish: {}", received_clients);
    // // let _ = state.tx.send(msg);

    // // Remove username from map so new clients can take it.
    // state.user_set.lock().unwrap().remove(&username);
}

// Include utf-8 file at **compile** time.
async fn index() -> Html<&'static str> {
    Html(include_str!("../static/chat.html"))
}
