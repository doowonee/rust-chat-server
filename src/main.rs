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
use futures::{
    stream::{SplitStream, StreamExt},
    FutureExt,
};
use session::Session;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::mpsc::{self, UnboundedSender},
    time::Instant,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::protocol::{
    AuthRequest, EnterRoomRequest, LeaveRoomRequest, PacketKind, Ping, Request, SendTextRequest,
};

// Our shared state
pub struct AppState {
    /// 세션 아이디를 키로 하고 세션 구조체를 값으로 하는 맵
    sessions: Mutex<BTreeMap<String, Session>>,
    /// 유저 아이디를 키로하고 세션 아이디를 값으로 하는 트리맵
    users: Mutex<BTreeMap<String, String>>,
    /// 채팅방 아이디를 키로하고 세션 아디 셋을 값으로 하는 트리맵
    rooms: Mutex<BTreeMap<String, BTreeSet<String>>>,
    redis_client: RedisClient,
    redis_subscriber: SubscriberClient,
}

/// pub sub 대상 채널 이름
pub const REDIS_CHANNEL_NAME: &str = "zxc";

/// ubounded channel 로 웹소켓 송신 채널로 포워딩 된다
pub type WebsocketTx = UnboundedSender<Result<Message, Error>>;
/// 웹소켓 수신 채널
pub type WebsocketRx = SplitStream<WebSocket>;

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
    let rooms = Mutex::new(BTreeMap::new());
    // 웹 서버 핸들러간 공유
    let app_state = Arc::new(AppState {
        sessions,
        users,
        rooms,
        redis_client,
        redis_subscriber,
    });

    // 웹 소켓 수신 세션별로 subscirbe가 아니라 단일 레디스 수신 핸들러임
    let _redis_reieve_handler = tokio::spawn(on_message(app_state.clone()));

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
    let (sender, receiver) = stream.split();

    // 버퍼가 무제한인 채널을 만든다 메시지 처리 밀리면 메모리 부족해짐
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

    // This task will receive messages from client and send them to broadcast subscribers.
    // 웹소켓으로 부터 데이터를 수신하면 redis로 publish
    let recv_task = tokio::spawn(on_rx(sid.clone(), state.clone(), ub_tx.clone(), receiver));
    // 클라가 연결으르 종료하지 않는 이상 수신 소켓 핸들러가 무한루프로 실행된다
    recv_task.await.unwrap();

    // 클라 사이드에서 연결 강제 종료 된것으로 세션 정보 삭제
    // 연결 종료시 세션 정보 삭제
    let mut sessions = state.sessions.lock().unwrap();
    match sessions.get(&sid) {
        Some(v) => {
            let mut users = state.users.lock().unwrap();
            users.remove(&v.user().map(|v| v.id.clone()).unwrap_or_default());
            sessions.remove(&sid);

            // 채팅방 전체를 순회해서 해당 세션 ID 삭제
            // @TODO O(N) 복잡도로 채팅방이 나중에 많아지면 오버헤드가 큼 최적화 필요
            let mut rooms = state.rooms.lock().unwrap();
            for session_ids in rooms.values_mut() {
                session_ids.remove(&sid);
            }
            tracing::info!("{} 세션 제거 완료", sid);
        }
        None => {
            tracing::debug!("{} 세선 정보가 없음 그냥 종료", sid);
        }
    };
}

// Include utf-8 file at **compile** time.
async fn index() -> Html<&'static str> {
    Html(include_str!("../static/chat.html"))
}

/// 버퍼링 기준 MS
const BUFFERING_PERIOD_MS: u64 = 50;

/// 레디스로 부터 subscribe 될 때
async fn on_message(state: Arc<AppState>) {
    let redis_subscriber = state.redis_subscriber.clone();
    let mut message_stream = redis_subscriber.on_message();
    // redis 로부터 받은 메시지를 임시 저장하는 버퍼
    let mut msg_buffer: VecDeque<String> = VecDeque::new();
    let mut last_sent_at = Instant::now();
    while let Some((channel, message)) = message_stream.next().await {
        let stop_watch = Instant::now();
        // 레디스로 부터 메시지 수신시 일단 메시지 버퍼에 저장
        let msg = message.as_string().unwrap_or_default();
        tracing::debug!("on_message 수신 {:?} on channel {}", msg, channel);
        msg_buffer.push_back(msg);
        if last_sent_at.elapsed() < Duration::from_millis(BUFFERING_PERIOD_MS) {
            // 마지막 송신 시간 BUFFERING_PERIOD_MS 미만이면 송신 작업 안함
            tracing::debug!(
                "최근 발송 시간 보다 {}ms 미만으로 발송 안함 버퍼링",
                BUFFERING_PERIOD_MS
            );
            continue;
        }
        let rooms = state.rooms.lock().unwrap();
        let sessions = state.sessions.lock().unwrap();
        // 발송 행위 시작 시간 갱신
        last_sent_at = Instant::now();
        // 버퍼에서 앞에서 부터 꺼내와서 송신 작업 처리
        while let Some(msg) = msg_buffer.pop_front() {
            let json: serde_json::Value = serde_json::from_str(&msg).unwrap_or_default();
            let room_id = json["r"].as_str().unwrap_or_default();
            match rooms.get(room_id) {
                Some(sessions_in_room) => {
                    for session_id in sessions_in_room {
                        match sessions.get(session_id) {
                            Some(session) => {
                                // 해당 세션 웹소켓 송신용 스트림으로 메시지 송신
                                session.send(msg.clone());
                            }
                            None => {
                                tracing::warn!(
                                    "on_message 없는 세션 {} 송신 못했음 {:?}",
                                    session_id,
                                    msg
                                );
                            }
                        }
                    }
                }
                None => {
                    tracing::warn!("on_message 없는 room id 임 그냥 무시 {:?}", message);
                }
            }
        }
        tracing::info!(
            "버퍼 순회해서 전파 처리시간 {:?} {:?}",
            stop_watch.elapsed(),
            message
        );
    }
}

async fn on_rx(sid: String, state: Arc<AppState>, ws_tx: WebsocketTx, mut ws_rx: WebsocketRx) {
    while let Some(Ok(message)) = ws_rx.next().await {
        if let Message::Text(payload) = message {
            let stop_watch = Instant::now();
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

                    req.handle(sid.clone(), ws_tx.clone(), now).await;
                }
                PacketKind::AuthRequest => {
                    let req: AuthRequest = match serde_json::from_value(packet.payload) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::error!("Wrong AuthRequest: {} payload: {}", e, payload);
                            return;
                        }
                    };

                    req.handle(sid.clone(), ws_tx.clone(), state.clone()).await;
                }
                PacketKind::SendTextRequest => {
                    let req: SendTextRequest = match serde_json::from_value(packet.payload) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::error!("Wrong SendTextRequest: {} payload: {}", e, payload);
                            return;
                        }
                    };

                    req.handle(sid.clone(), ws_tx.clone(), state.clone()).await;
                }
                PacketKind::EnterRoomRequest => {
                    let req: EnterRoomRequest = match serde_json::from_value(packet.payload) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::error!("Wrong EnterRoomRequest: {} payload: {}", e, payload);
                            return;
                        }
                    };

                    req.handle(sid.clone(), ws_tx.clone(), state.clone()).await;
                }
                PacketKind::LeaveRoomRequest => {
                    let req: LeaveRoomRequest = match serde_json::from_value(packet.payload) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::error!("Wrong LeaveRoomRequest: {} payload: {}", e, payload);
                            return;
                        }
                    };

                    req.handle(sid.clone(), ws_tx.clone(), state.clone()).await;
                }
                _ => {
                    tracing::error!("{:?} Wrong Request {}", packet.op_code, payload);
                    return;
                }
            };
            tracing::info!(
                "수신 처리 완료 처리시간 {:?} {:?}",
                stop_watch.elapsed(),
                payload
            );
        }
    }
}
