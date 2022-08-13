//! websocket session

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
}

impl Session {
    pub fn new(sid: &str, tx: UnboundedSender<Result<Message, Error>>) -> Session {
        Session {
            id: sid.into(),
            tx,
            user_info: None,
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
