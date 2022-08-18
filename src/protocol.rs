//! protocol
//!
//! serde rename으로 통신시 필드명을 줄여서 패킷 크기 줄이기
//! op_code를 enum 화 하여 optional 한 필드로 단일 struct
//! 가 아니라 enum 즉 프로토콜 타입별로 struct를 사용

use std::{collections::BTreeSet, sync::Arc};

use axum::extract::ws::Message;
use chrono::prelude::*;
use num_enum::TryFromPrimitive;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::{session::User, AppState, WebsocketTx};

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
    /// 채팅방 입장
    EnterRoomRequest = 181,
    EnterRoomSuccess = 182,
    EnterRoomFail = 183,
    LeaveRoomRequest = 191,
    LeaveRoomSuccess = 192,
    LeaveRoomFail = 193,
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
    pub async fn handle(&self, _sid: String, tx: WebsocketTx, now: NaiveDateTime) {
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
    #[serde(rename = "r")]
    pub room_id: String,
}

impl SendTextRequest {
    /// 요청 패킷 처리
    pub async fn handle(&self, sid: String, tx: WebsocketTx, state: Arc<AppState>) {
        // 함수가 끝나면 자동 lock 해제
        let sessions = state.sessions.lock().unwrap();
        let rooms = state.rooms.lock().unwrap();

        if !rooms.contains_key(&self.room_id) {
            // 존재 하지 않는 채팅방이므로 실패 처리
            // 누군가 입장이 되어있는 상태여야 전파 가능함
            // 인증 안되서 전파 실패 처리
            // @TODO 서버 클러스터 기능을 위해 이건 정보들은 레디스에 중앙에서 관리해야함
            tracing::error!("존재 하지 않는 채팅방이라 전파 실패 {} {:?}", sid, self);
            let json = serde_json::json!({
                "o": PacketKind::SendTextFail,
                "s": sid,
            });
            let msg = format!("{}", json);
            // 송신자에게만 전파
            let _ = tx.send(Ok(Message::Text(msg)));
            return;
        }
        match sessions.get(&sid) {
            Some(v) => {
                match v.user() {
                    Some(u) => {
                        // 인증 된 세션이라 채팅 전파
                        let json = serde_json::json!({
                            "o": PacketKind::SendTextSuccess,
                            "c": self.content,
                            "r": self.room_id,
                            "n": u.name,
                            "i": u.id,
                        });
                        let msg = format!("{}", json);

                        // redis publish용 mpsc 로 send
                        tracing::debug!("redis_publish_tx 보냄 {} {}", sid, msg);
                        let _ = state.redis_publish_tx.send(msg);
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

#[derive(Serialize, Deserialize, Debug)]
pub struct EnterRoomRequest {
    #[serde(rename = "r")]
    pub room_id: String,
}

impl EnterRoomRequest {
    /// 요청 패킷 처리
    pub async fn handle(&self, sid: String, tx: WebsocketTx, state: Arc<AppState>) {
        // 함수가 끝나면 자동 lock 해제
        let sessions = state.sessions.lock().unwrap();
        let mut rooms = state.rooms.lock().unwrap();

        if sessions
            .get(&sid)
            .map(|v| v.user().is_none())
            .unwrap_or(false)
        {
            // 인증 안된 세션이므로 입장 거부 처리
            tracing::error!("인증 안된 세션이라 입장 실패 {}", sid);
            let json = serde_json::json!({
                "o": PacketKind::EnterRoomFail,
                "r": self.room_id,
            });
            let msg = format!("{}", json);
            // 송신자에게만 전파
            let _ = tx.send(Ok(Message::Text(msg)));
            return;
        }

        match rooms.get_mut(&self.room_id) {
            Some(room) => {
                // 이미 있는 채팅방이면 입장만 처리
                // 이미 입장되어있는 세션이어도 set 이라 그냥 입장 된것으로 처리 에러 안줌
                room.insert(sid);
            }
            None => {
                // 채팅방 생성 후 입장 처리
                rooms.insert(self.room_id.to_owned(), BTreeSet::from([sid]));
            }
        };

        // 인증 된 세션이라 입장 허용
        let json = serde_json::json!({
            "o": PacketKind::EnterRoomSuccess,
            "r": self.room_id,
        });
        let msg = format!("{}", json);
        // 송신자에게만 전파
        let _ = tx.send(Ok(Message::Text(msg)));
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LeaveRoomRequest {
    #[serde(rename = "r")]
    pub room_id: String,
}

impl LeaveRoomRequest {
    /// 요청 패킷 처리
    pub async fn handle(&self, sid: String, tx: WebsocketTx, state: Arc<AppState>) {
        // 함수가 끝나면 자동 lock 해제
        let mut rooms = state.rooms.lock().unwrap();
        match rooms.get_mut(&self.room_id) {
            Some(room) => {
                // 채팅방 키에서 세션 아이디 제거
                room.remove(&sid);
            }
            None => {
                // 존재 하지 않는 채팅방 퇴장을 요청함 근데 그냥 성공 반환
                tracing::warn!(
                    "존재 하지 않는 채팅방 퇴장 요청임 근데 그냥 성공 반환 {} {:?}",
                    sid,
                    self
                );
            }
        };

        let json = serde_json::json!({
            "o": PacketKind::LeaveRoomSuccess,
            "r": self.room_id,
        });
        let msg = format!("{}", json);
        // 송신자에게만 전파
        let _ = tx.send(Ok(Message::Text(msg)));
    }
}
