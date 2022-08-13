//! protocol
//!
//! serde rename으로 통신시 필드명을 줄여서 패킷 크기 줄이기
//! op_code를 enum 화 하여 optional 한 필드로 단일 struct
//! 가 아니라 enum 즉 프로토콜 타입별로 struct를 사용

use serde::{Deserialize, Serialize};

use crate::error::{self, Error};

/// 프로토콜
#[derive(Debug, Serialize, Deserialize)]
pub enum PacketKind {
    Ping(Ping),
    Pong(Pong),
}

impl PacketKind {
    /// 프로토콜에 따른 숫자 코드 값을 반환 한다.
    pub fn as_code(&self) -> i16 {
        match self {
            PacketKind::Ping(_) => 1,
            PacketKind::Pong(_) => 2,
        }
    }
    /// 요청 프로토콜 여부
    pub fn is_request(&self) -> bool {
        match self {
            PacketKind::Ping(_) => true,
            PacketKind::Pong(_) => false,
        }
    }
}

impl TryFrom<&str> for PacketKind {
    type Error = error::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        // JSON format check
        let json_value: serde_json::Value = match serde_json::from_str(value) {
            Ok(v) => v,
            Err(e) => {
                return Err(Error::ProtocolInvalid(format!(
                    "json parsing failed: {}",
                    e
                )));
            }
        };

        // op code field name type check
        let op_code = match json_value["o"].as_i64() {
            Some(v) => match i16::try_from(v) {
                Ok(v) => v,
                Err(e) => {
                    return Err(Error::ProtocolInvalid(format!("op_code wrong: {}", e)));
                }
            },
            None => {
                return Err(Error::ProtocolInvalid(format!(
                    "op_code wrong: {}",
                    json_value
                )));
            }
        };

        let paylaod = match op_code {
            1 => {
                let payload: Ping = match serde_json::from_value(json_value) {
                    Ok(v) => v,
                    Err(e) => {
                        return Err(Error::ProtocolInvalid(format!(
                            "op_code and payload not matched: {}",
                            e
                        )));
                    }
                };
                PacketKind::Ping(payload)
            }
            2 => {
                let payload: Pong = match serde_json::from_value(json_value) {
                    Ok(v) => v,
                    Err(e) => {
                        return Err(Error::ProtocolInvalid(format!(
                            "op_code and payload not matched: {}",
                            e
                        )));
                    }
                };
                PacketKind::Pong(payload)
            }
            _ => {
                return Err(Error::ProtocolInvalid(format!(
                    "op_code wrong: {}",
                    json_value
                )));
            }
        };
        Ok(paylaod)
    }
}

/// 통신 상태 체크 요청
#[derive(Serialize, Deserialize, Debug)]
pub struct Ping {
    /// op code
    #[serde(rename = "o")]
    op_code: i16,
    /// 클라의 현재 시간 epoch
    #[serde(rename = "t")]
    client_epoch: i64,
}

/// 통신 상태 체크 응답
#[derive(Serialize, Deserialize, Debug)]
pub struct Pong {
    /// op code
    #[serde(rename = "o")]
    pub op_code: i16,
    /// 서버의 현재 시간 epoch
    #[serde(rename = "t")]
    pub server_epoch: i64,
}
