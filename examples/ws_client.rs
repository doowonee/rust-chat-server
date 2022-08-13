//! websocket client program
//!
//! connect massive web socket client and broadcast message for benchmarking

use futures::{pin_mut, SinkExt, StreamExt};
use log::info;
use rand::prelude::*;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::{self, sleep};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const WS_ENDPOINT: &str = "ws://localhost:3000/websocket";

#[derive(Debug)]
pub struct Status(pub i64, pub i64, pub u32);

#[tokio::main]
async fn main() {
    // RUST_LOG 가 없으면 기본 레벨 지정
    env_logger::builder()
        .parse_filters(&std::env::var("RUST_LOG").unwrap_or_else(|_| "debug".into()))
        .format_timestamp_millis()
        .init();

    // 스레드간 공유되는 상태로 수신 메시지 갯수 송신 메시지 갯수 클라이언트 갯수
    let mut status: Arc<Mutex<Status>> = Arc::new(Mutex::new(Status(0, 0, 0)));

    // 웹소켓 클라 만들어서 인증 하고 채팅 전송 그리고 수신시 수신 메시지 갯수 더하기
    let (ws_stream, _) = connect_async(WS_ENDPOINT).await.expect("Failed to connect");

    let (mut write, mut read) = ws_stream.split();

    info!("수신 시작");
    let cloned_status = status.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(_msg)) = read.next().await {
            cloned_status.lock().unwrap().0 += 1;
        }
    });

    // 인증 유저 아이디 랜덤으로 다르게 지정
    let msg = format!(
        r#"{{"o":10,"n":"user_name","i":"s{}"}}"#,
        rand::random::<i64>()
    );
    write.send(Message::from(msg)).await.unwrap();

    // 0.3초 마다 랜덤 숫자 넣은 채팅 전송
    let cloned_status = status.clone();
    let mut send_task = tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_millis(30));
        loop {
            interval.tick().await;
            let msg = format!(
                r#"{{"o":101,"c":"{} 안녕 나는 {} 이야"}}"#,
                rand::random::<i64>(),
                rand::random::<i64>()
            );
            write.send(Message::from(msg)).await.unwrap();
            cloned_status.lock().unwrap().1 += 1;
        }
    });

    let cloned_status = status.clone();
    let mut show_status = tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(3));
        loop {
            interval.tick().await;
            info!("status: {:?}", cloned_status.lock().unwrap());
        }
    });

    recv_task.await.unwrap();
    send_task.await.unwrap();
    show_status.await.unwrap();
}
