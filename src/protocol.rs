//! protocol
//!
//! serde rename으로 통신시 필드명을 줄여서 패킷 크기 줄이기
//! op_code를 enum 화 하여 optional 한 필드로 단일 struct
//! 가 아니라 enum 즉 프로토콜 타입별로 struct를 사용

use num_enum::TryFromPrimitive;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};

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

/// 통신 상태 체크 응답
#[derive(Serialize, Deserialize, Debug)]
pub struct Pong {
    /// epoch from client
    #[serde(rename = "t1")]
    pub client_epoch: i64,
    /// epoch when received from client
    #[serde(rename = "t2")]
    pub received_epoch: i64,
    /// epoch when send from server
    #[serde(rename = "t3")]
    pub server_epoch: i64,
}

/// 인증 요청
#[derive(Serialize, Deserialize, Debug)]
pub struct AuthRequest {
    #[serde(rename = "n")]
    pub user_name: String,
    #[serde(rename = "i")]
    pub user_id: String,
}

/// 인증 성공
#[derive(Serialize, Deserialize, Debug)]
pub struct AuthSuccess {
    #[serde(rename = "n")]
    pub user_name: String,
    #[serde(rename = "i")]
    pub user_id: String,
    #[serde(rename = "s")]
    pub session_id: String,
}

/// 인증 실패
#[derive(Serialize, Deserialize, Debug)]
pub struct AuthFail {
    #[serde(rename = "s")]
    pub session_id: String,
}
