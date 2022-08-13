//! protocol
//!
//! serde rename으로 통신시 필드명을 줄여서 패킷 크기 줄이기
//! op_code를 enum 화 하여 optional 한 필드로 단일 struct
//! 가 아니라 enum 즉 프로토콜 타입별로 struct를 사용

use std::sync::Arc;

use axum::{extract::ws::Message, Error};
use chrono::prelude::*;
use fred::prelude::PubsubInterface;
use num_enum::TryFromPrimitive;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use tokio::sync::mpsc::UnboundedSender;

use crate::{session::User, AppState, WebsocketTx, REDIS_CHANNEL_NAME};

/// 프로토콜
#[derive(Debug, TryFromPrimitive, Serialize_repr, Deserialize_repr)]
#[repr(i16)]
pub enum PacketKind {
    Ping = 1,
    Pong = 2,
    AuthRequest = 10,
    AuthSuccess = 11,
    AuthFail = 12,
    SendTextRequest = 101,
    SendTextSuccess = 102,
    /// 인증 정보 누락으로 실패 처리
    SendTextFail = 103,
}

// @TODO 응답에 op_code 추가
// pub fn make_response<T: Serialize>(packet_kind: PacketKind, T) -> String {
//     serde_json::to_string(&T).unwrap_or_default(),
// }

/// 요청 패킷 구조
#[derive(Serialize, Deserialize, Debug)]
pub struct Request {
    /// op code
    #[serde(rename = "o")]
    pub op_code: PacketKind,
    /// 페이로드
    #[serde(flatten)]
    pub payload: serde_json::Value,
}

/// 응답 패킷 구조
#[derive(Serialize, Deserialize, Debug)]
pub struct Response {
    /// 전체 전파 해야할 내용인지 송신자에게만 전달해야할 내용인지 여부
    pub is_broad_cast: bool,
    /// 페이로드
    pub payload: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Ping {
    /// epoch from client
    #[serde(rename = "t")]
    pub client_epoch: i64,
}

impl Ping {
    /// 요청 패킷 처리
    pub async fn handle(&self, sid: String, tx: WebsocketTx, now: NaiveDateTime) {
        let json = serde_json::json!({
            "o": PacketKind::Pong,
            "t1": &self.client_epoch,
            "t2": now.timestamp_millis(),
            "t3": Utc::now().timestamp_millis(),
        });
        let msg = format!("{}", json);
        // 송신자에게만 전파
        let _ = tx.send(Ok(Message::Text(msg)));
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AuthRequest {
    #[serde(rename = "n")]
    pub user_name: String,
    #[serde(rename = "i")]
    pub user_id: String,
}

impl AuthRequest {
    /// 요청 패킷 처리
    pub async fn handle(&self, sid: String, tx: WebsocketTx, state: Arc<AppState>) {
        // 함수가 끝나면 자동 lock 해제
        let mut users = state.users.lock().unwrap();
        let mut sessions = state.sessions.lock().unwrap();

        // 일단 중복 아이디일 경우 인증 실패 처리
        if !users.contains_key(&self.user_id) {
            if let Some(session) = sessions.get_mut(&sid) {
                // 유저 아이이디에 해당 세션 아이디를 넣고 유저정보에도 인증 정보를 담는다
                users.insert(self.user_id.clone(), sid.clone());
                let user_info = User {
                    id: self.user_id.clone(),
                    name: self.user_name.clone(),
                };
                session.auth(user_info);
                let json = serde_json::json!({
                    "o": PacketKind::AuthSuccess,
                    "n": self.user_name,
                    "i": self.user_id,
                    "s": sid,
                });
                let msg = format!("{}", json);
                // 송신자에게만 전파
                let _ = tx.send(Ok(Message::Text(msg)));
            } else {
                tracing::error!("테스트용 인증 실패: 세션 정보에 해당 세션이 없음 {}", sid);
                let json = serde_json::json!({
                    "o": PacketKind::AuthFail,
                    "n": self.user_name,
                    "i": self.user_id,
                    "s": sid,
                });
                let msg = format!("{}", json);
                // 송신자에게만 전파
                let _ = tx.send(Ok(Message::Text(msg)));
            }
        } else {
            tracing::error!("테스트용 인증 실패: 이미 인증된 세션임 {}", sid);
            let json = serde_json::json!({
                "o": PacketKind::AuthFail,
                "n": self.user_name,
                "i": self.user_id,
                "s": sid,
            });
            let msg = format!("{}", json);
            // 송신자에게만 전파
            let _ = tx.send(Ok(Message::Text(msg)));
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SendTextRequest {
    #[serde(rename = "c")]
    pub content: String,
}

impl SendTextRequest {
    /// 요청 패킷 처리
    pub async fn handle(&self, sid: String, tx: WebsocketTx, state: Arc<AppState>) {
        // 함수가 끝나면 자동 lock 해제
        let sessions = state.sessions.lock().unwrap();

        match sessions.get(&sid) {
            Some(v) => {
                match v.user() {
                    Some(u) => {
                        // 인증 된 세션이라 채팅 전파
                        let json = serde_json::json!({
                            "o": PacketKind::SendTextSuccess,
                            "c": self.content,
                            "n": u.name,
                            "i": u.id,
                        });
                        let msg = format!("{}", json);

                        // redis publish로 브로드 캐스팅
                        let redis_client = state.redis_client.clone();
                        // await를 할수 없어서 task 만들어서 비동기로 레디스에 publish
                        tokio::spawn(async move {
                            let received_clients: i64 = redis_client
                                .publish(REDIS_CHANNEL_NAME, &msg)
                                .await
                                .unwrap();
                            tracing::debug!(
                                "SendTextRequest 성공 {} {} {}",
                                received_clients,
                                sid,
                                msg
                            );
                        });
                    }
                    None => {
                        // 인증 안되서 전파 실패 처리
                        tracing::error!("인증 안된 세션이라 채팅 전파 실패 {}", sid);
                        let json = serde_json::json!({
                            "o": PacketKind::SendTextFail,
                            "s": sid,
                        });
                        let msg = format!("{}", json);
                        // 송신자에게만 전파
                        let _ = tx.send(Ok(Message::Text(msg)));
                    }
                }
            }
            None => {
                tracing::error!("SendTextRequest 실패: 세션 정보에 해당 세션이 없음 {}", sid);
                let json = serde_json::json!({
                    "o": PacketKind::SendTextFail,
                    "s": sid,
                });
                let msg = format!("{}", json);
                // 송신자에게만 전파
                let _ = tx.send(Ok(Message::Text(msg)));
            }
        };
    }
}
