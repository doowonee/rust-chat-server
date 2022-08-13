//! websocket session

use bson::oid::ObjectId;

pub fn generate_id() -> String {
    format!("ws{}", ObjectId::new())
}

// 유저아디랑 세션이랑 구분
// 세션별로 채팅룸별로 전달 해야되나 안되나 구분
// 송 수신 채널 분리
