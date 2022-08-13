//! protocol
//!
//! serde rename으로 통신시 필드명을 줄여서 패킷 크기 줄이기
//! op_code를 enum 화 하여 optional 한 필드로 단일 struct
//! 가 아니라 enum 즉 프로토콜 타입별로 struct를 사용

use std::sync::Arc;

use chrono::prelude::*;
use num_enum::TryFromPrimitive;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::AppState;

/// 프로토콜
#[derive(Debug, TryFromPrimitive, Serialize_repr, Deserialize_repr)]
#[repr(i16)]
pub enum PacketKind {
    Ping = 1,
    Pong = 2,
    AuthRequest = 10,
    AuthSuccess = 11,
    AuthFail = 12,
}

// @TODO 응답에 op_code 추가
// pub fn make_response<T: Serialize>(packet_kind: PacketKind, T) -> String {
//     serde_json::to_string(&T).unwrap_or_default(),
// }

/// 통신 패킷 체크 응답
#[derive(Serialize, Deserialize, Debug)]
pub struct Packet {
    /// op code
    #[serde(rename = "o")]
    pub op_code: PacketKind,
    /// 페이로드
    #[serde(flatten)]
    pub payload: serde_json::Value,
}

/// 통신 상태 체크 요청
#[derive(Serialize, Deserialize, Debug)]
pub struct Ping {
    /// epoch from client
    #[serde(rename = "t")]
    pub client_epoch: i64,
}

impl Ping {
    /// 요청 패킷 처리
    pub async fn handle(&self, now: NaiveDateTime) -> String {
        let json = serde_json::json!({
            "o": PacketKind::Pong,
            "t1": &self.client_epoch,
            "t2": now.timestamp_millis(),
            "t3": Utc::now().timestamp_millis(),
        });
        format!("{}", json)
    }
}

/// 인증 요청
#[derive(Serialize, Deserialize, Debug)]
pub struct AuthRequest {
    #[serde(rename = "n")]
    pub user_name: String,
    #[serde(rename = "i")]
    pub user_id: String,
}

impl AuthRequest {
    /// 요청 패킷 처리
    pub async fn handle(&self, sid: &str, state: &Arc<AppState>) -> String {
        // 함수가 끝나면 자동 lock 해제
        let mut user_set = state.user_set.lock().unwrap();

        // 일단 중복 아이디일 경우 인증 실패 처리
        let json = if !user_set.contains(&self.user_id) {
            user_set.insert(self.user_id.to_owned());
            serde_json::json!({
                "o": PacketKind::AuthSuccess,
                "n": self.user_name,
                "i": self.user_id,
                "s": sid,
            })
        } else {
            tracing::error!("테스트용 인증 실패 {}", sid);
            serde_json::json!({
                "o": PacketKind::AuthFail,
                "n": self.user_name,
                "i": self.user_id,
                "s": sid,
            })
        };
        format!("{}", json)
    }
}
