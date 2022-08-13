//! websocket session

use std::collections::BTreeSet;

use axum::{extract::ws::Message, Error};
use bson::oid::ObjectId;
use tokio::sync::mpsc::UnboundedSender;

use crate::{AppState, WebsocketTx};

pub fn generate_id() -> String {
    format!("ws{}", ObjectId::new())
}

/// 세션 정보로 인증된 유저면 유저 정보가 존재
pub struct Session {
    id: String,
    tx: UnboundedSender<Result<Message, Error>>,
    user_info: Option<User>,
    rooms: BTreeSet<String>,
}

impl Session {
    pub fn new(sid: &str, tx: UnboundedSender<Result<Message, Error>>) -> Session {
        Session {
            id: sid.into(),
            tx,
            user_info: None,
            rooms: BTreeSet::new(),
        }
    }

    /// 인증 정보를 저장 한다
    pub fn auth(&mut self, user_info: User) {
        self.user_info = Some(user_info);
    }

    /// 유저정보 조회
    pub fn user(&self) -> Option<&User> {
        self.user_info.as_ref()
    }

    /// room에 조인
    pub fn join(&mut self, room_id: &str) {
        self.rooms.insert(room_id.to_owned());
    }

    /// room에 에서 제거
    pub fn leave(&mut self, room_id: &str) {
        self.rooms.remove(room_id);
    }

    /// room에 조인
    pub fn is_joined(&self, room_id: &str) -> bool {
        self.rooms.contains(room_id)
    }
}

/// 인증시 생성되는 유저 정보
pub struct User {
    pub id: String,
    pub name: String,
}

pub fn add_session(state: &AppState, sid: &str, tx: WebsocketTx) {
    let mut sessions = state.sessions.lock().unwrap();
    sessions.insert(sid.to_owned(), Session::new(sid, tx));
}
